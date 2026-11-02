//! One CallSQL statement at a time against a store: the core of the REPL and,
//! from M3, of the model's turn loop.

use crate::live::LiveSink;
use crate::sink::StoreSink;
use crate::RenderOptions;
use kleene_algebra::{plan as plan_calls, CostModel};
use kleene_core::{Batch, BudgetUsage, Catalog, SessionId, StatementId};
use kleene_exec::{execute_statement, ExecContext, ExecError, StatementResult};
use kleene_llm::Provider;
use kleene_sql::{parse, plan, plan_sql, render_error, Statement, StatementKind};
use kleene_store::{DuckDbStore, Store};
use kleene_tools::{catalog_entries, standard_tools, ToolContext, WebSearchBackend};
use kleene_trace::{TraceEvent, Tracer};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Runs statements and renders results as the model (or a human) sees them.
pub struct Repl {
    sink: Arc<LiveSink>,
    render: RenderOptions,
    session: SessionId,
    tracer: Option<Tracer>,
}

enum Pending {
    Ast(Box<sqlparser::ast::Statement>, String),
    /// A CallSQL addition the parser does not know (`FINAL`, `CALL`,
    /// `CREATE FUNCTION`, `CREATE AGENT`), kept as text and planned when its
    /// turn comes.
    Extension(String),
}

fn error(text: String) -> Rendered {
    Rendered {
        text,
        is_error: true,
        is_final: false,
        rows: None,
    }
}

/// The rendering of one statement's outcome.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rendered {
    /// Text to show.
    pub text: String,
    /// The statement failed.
    pub is_error: bool,
    /// `FINAL` was reached.
    pub is_final: bool,
    /// The `FINAL` relation itself (only set when `is_final`).
    pub rows: Option<kleene_core::Batch>,
}

/// What a REPL is built from.
pub struct ReplConfig {
    /// Workspace root for tools.
    pub workspace: PathBuf,
    /// Model provider, if any.
    pub provider: Option<Arc<dyn Provider>>,
    /// Trace sink, if any (the store's own sink is the usual choice).
    pub tracer: Option<Tracer>,
    /// The service behind `web_search`, if any.
    pub web_search: Option<WebSearchBackend>,
    /// Which tools the session may use (`--read-only`, `--no-shell`,
    /// `--no-web`, `--tools`).
    pub profile: kleene_core::ToolProfile,
}

impl Repl {
    /// Open a REPL over a store with the standard catalog, no provider and
    /// tools rooted at the current directory.
    pub async fn new(store: DuckDbStore) -> Result<Self, kleene_store::StoreError> {
        Self::with_config(
            store,
            ReplConfig {
                workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                provider: None,
                tracer: None,
                web_search: None,
                profile: kleene_core::ToolProfile::default(),
            },
        )
        .await
    }

    /// Open a REPL with a provider, a workspace and a tracer.
    pub async fn with_config(
        store: DuckDbStore,
        cfg: ReplConfig,
    ) -> Result<Self, kleene_store::StoreError> {
        let mut catalog = kleene_core::standard_catalog();
        cfg.profile.apply(&mut catalog);
        let store_sink = StoreSink::new(store, catalog).await?;
        let tools = Arc::new(standard_tools());
        let entries: Vec<_> = catalog_entries(&tools)
            .into_iter()
            .filter(|e| cfg.profile.allows(e))
            .collect();
        let live = LiveSink::new(
            store_sink,
            cfg.provider,
            tools,
            ToolContext::new(cfg.workspace).with_web_search(cfg.web_search),
            cfg.tracer.clone(),
        );
        live.register_tool_catalog(entries).await;
        Ok(Self {
            sink: Arc::new(live),
            render: RenderOptions::default(),
            session: SessionId::new(),
            tracer: cfg.tracer,
        })
    }

    /// A REPL over an existing live sink (the harness builds the sink itself
    /// so children get namespaces, roles and budget slices).
    pub fn from_sink(
        sink: Arc<LiveSink>,
        session: SessionId,
        tracer: Option<Tracer>,
        render: RenderOptions,
    ) -> Self {
        Self {
            sink,
            render,
            session,
            tracer,
        }
    }

    /// The live sink (settings, budget, functions).
    pub fn sink(&self) -> &Arc<LiveSink> {
        &self.sink
    }

    /// Create `name` from `batch` (`kleene repl --load`), replacing a table
    /// of that name.
    pub async fn load_table(&self, name: &str, batch: Batch) -> Result<(), ExecError> {
        crate::session::load_table(&self.sink, name, batch).await
    }

    /// The live catalog.
    pub fn catalog(&self) -> Arc<tokio::sync::RwLock<Catalog>> {
        self.sink.catalog()
    }

    /// This REPL's session id (trace attribution).
    pub fn session(&self) -> SessionId {
        self.session
    }

    /// Use a caller-chosen session id (the daemon keys interactive sessions
    /// by the id the client picked).
    pub fn set_session(&mut self, id: SessionId) {
        self.session = id;
    }

    /// Rendering options.
    pub fn render_options_mut(&mut self) -> &mut RenderOptions {
        &mut self.render
    }

    /// A cost model seeded with the store's row counts.
    pub async fn cost_model(&self) -> CostModel {
        let mut cost = CostModel::default();
        let store = self.sink.store().store();
        if let Ok(tables) = store.tables().await {
            for t in tables {
                if let Ok(b) = store
                    .query(&format!(
                        "SELECT COUNT(*) FROM \"{}\"",
                        t.replace('"', "\"\"")
                    ))
                    .await
                {
                    if let Some(n) = b.rows.first().and_then(|r| r[0].as_int()) {
                        cost.table_rows.insert(t.to_ascii_lowercase(), n as f64);
                    }
                }
            }
        }
        if let Some(r) = self.sink.settings().await.max_recursion_rounds {
            cost.recursion_rounds = r as f64;
        }
        // What child sessions cost here: the finished children in the
        // trace, once there are a few, replace the defaults.
        if let Ok(b) = store
            .query(
                "SELECT COUNT(*), AVG(calls), AVG(tokens), AVG(dollars) FROM trace_sessions WHERE depth > 0 AND finished_at IS NOT NULL",
            )
            .await
        {
            if let Some(row) = b.rows.first() {
                let n = row[0].as_int().unwrap_or(0);
                if n >= 3 {
                    if let (Some(calls), Some(tokens)) = (row[1].as_f64(), row[2].as_f64()) {
                        cost.child_calls = calls.max(1.0);
                        cost.child_tokens = tokens.max(0.0);
                        cost.child_dollars = row[3].as_f64().filter(|d| *d > 0.0);
                        cost.child_learned = n as u64;
                    }
                }
            }
        }
        // What tokens cost here: the priced calls in the trace, once there
        // are a few, replace the default rate card, so a cheap model's
        // estimates, and the budget gate that trusts them, scale with it.
        // One actual cannot separate input from output rates, so the ratio
        // stays at the rate cards' five to one (cache reads at a tenth of
        // input) and the level is learned.
        if let Ok(b) = store
            .query(
                "SELECT COUNT(*), CAST(SUM(input_tokens) AS DOUBLE), CAST(SUM(cache_read_tokens) AS DOUBLE), CAST(SUM(output_tokens) AS DOUBLE), SUM(cost_usd) FROM trace_calls WHERE cost_usd > 0 AND error IS NULL AND NOT coalesce(memo_hit, false)",
            )
            .await
        {
            if let Some(row) = b.rows.first() {
                let n = row[0].as_int().unwrap_or(0);
                let input = row[1].as_f64().unwrap_or(0.0);
                let cached = row[2].as_f64().unwrap_or(0.0);
                let output = row[3].as_f64().unwrap_or(0.0);
                let dollars = row[4].as_f64().unwrap_or(0.0);
                let weighted = input + 0.1 * cached + 5.0 * output;
                if n >= 3 && weighted > 0.0 && dollars > 0.0 {
                    cost.usd_per_input_token = dollars / weighted;
                    cost.usd_per_output_token = 5.0 * cost.usd_per_input_token;
                    cost.prices_learned = n as u64;
                }
            }
        }
        // What earlier sessions learned (the store's `estimates` table),
        // then this session's own sample on top: the pass rate of every
        // boolean call predicate evaluated at least a few times, and the
        // rows per call of every table function called at least a few times.
        let (learned_sel, learned_branch) = self.sink.learned_estimates().await;
        for (name, (sel, n)) in learned_sel {
            if n >= 3 {
                cost.call_selectivity
                    .insert(name.clone(), sel.clamp(0.01, 0.99));
                cost.learned.insert(name, n);
            }
        }
        for (name, (branch, n)) in learned_branch {
            if n >= 3 {
                cost.call_branching.insert(name.clone(), branch.max(0.0));
                cost.learned.insert(name, n);
            }
        }
        for (name, (yes, total)) in self.sink.call_stats().await {
            if total >= 3 {
                cost.call_selectivity
                    .insert(name.clone(), (yes as f64 / total as f64).clamp(0.01, 0.99));
                cost.learned.insert(name, total);
            }
        }
        for (name, (rows, calls)) in self.sink.branch_stats().await {
            if calls >= 3 {
                cost.call_branching
                    .insert(name.clone(), rows as f64 / calls as f64);
                cost.learned.insert(name, calls);
            }
        }
        cost
    }

    /// Render `EXPLAIN` for a planned statement without executing it.
    pub async fn explain(&self, stmt: &Statement) -> Result<String, String> {
        let catalog = self.sink.catalog().read().await.clone();
        let cost = self.cost_model().await;
        let logical = match &stmt.kind {
            StatementKind::Query { plan }
            | StatementKind::Final { plan }
            | StatementKind::CreateTableAs { plan, .. }
            | StatementKind::Insert { plan, .. } => plan.clone(),
            StatementKind::Call { input: Some(p), .. } => p.clone(),
            other => return Err(format!("nothing to explain for {}", kind_name(other))),
        };
        let cp = plan_calls(&logical, &catalog, &cost);
        Ok(kleene_algebra::explain(&cp))
    }

    /// Plan and execute every statement in `sql`, rendering each outcome.
    ///
    /// Statements are planned one at a time, immediately before they run, so
    /// a table or function created by one statement is visible to the next.
    /// Execution stops at the first error or at `FINAL`.
    pub async fn submit(&self, sql: &str) -> Vec<Rendered> {
        let mut pending = vec![];
        for piece in split_statements(sql) {
            if is_extension(&piece) {
                // CallSQL additions the parser does not know are planned
                // when they run, like everything else, so a FINAL at the end
                // of a reply reads the tables the reply created.
                pending.push(Pending::Extension(piece));
                continue;
            }
            let asts = match parse(&piece) {
                Ok(a) => a,
                Err(e) => {
                    let mut out = self.run_pending(pending).await;
                    if out.last().is_none_or(|r| !r.is_error && !r.is_final) {
                        out.push(self.failed_to_plan(&piece, &e));
                    }
                    return out;
                }
            };
            let single = asts.len() == 1;
            pending.extend(asts.into_iter().enumerate().map(|(i, a)| {
                let text = if single && i == 0 {
                    piece.clone()
                } else {
                    a.to_string()
                };
                Pending::Ast(Box::new(a), text)
            }));
        }
        self.run_pending(pending).await
    }

    async fn run_pending(&self, pending: Vec<Pending>) -> Vec<Rendered> {
        let mut out = vec![];
        'items: for item in pending {
            let catalog = self.sink.catalog().read().await.clone();
            let stmts = match item {
                Pending::Ast(ast, text) => match plan(&ast, &catalog) {
                    Ok(mut s) => {
                        s.sql = text;
                        vec![s]
                    }
                    Err(e) => {
                        out.push(self.failed_to_plan(&text, &e));
                        break;
                    }
                },
                Pending::Extension(text) => match plan_sql(&text, &catalog) {
                    Ok(stmts) => stmts,
                    Err(e) => {
                        out.push(self.failed_to_plan(&text, &e));
                        break;
                    }
                },
            };
            for stmt in stmts {
                let rendered = self.run_one(&stmt).await;
                let stop = rendered.is_error || rendered.is_final;
                out.push(rendered);
                if stop {
                    break 'items;
                }
            }
        }
        out
    }

    /// Render a parse or planning failure, and trace it like any other failed
    /// statement so the transcript in `trace_statements` is complete.
    fn failed_to_plan(&self, sql: &str, e: &kleene_sql::SqlError) -> Rendered {
        let text = render_error(e);
        if let Some(t) = &self.tracer {
            let id = StatementId::new();
            t.emit(TraceEvent::StatementStarted {
                session: self.session,
                statement: id,
                sql: sql.to_string(),
            });
            t.emit(TraceEvent::StatementFinished {
                statement: id,
                rows: 0,
                usage: BudgetUsage::default(),
                error: Some(text.clone()),
                elapsed: std::time::Duration::ZERO,
            });
        }
        error(text)
    }

    async fn run_one(&self, stmt: &Statement) -> Rendered {
        let id = StatementId::new();
        let started = Instant::now();
        if let Some(t) = &self.tracer {
            t.emit(TraceEvent::StatementStarted {
                session: self.session,
                statement: id,
                sql: stmt.sql.clone(),
            });
        }
        self.sink.clear_cancel();
        self.sink.set_statement(Some(id)).await;
        let before = self.sink.usage().await;
        let memo_before = self.sink.memo_hits().await;
        let rendered = self.dispatch(stmt, id).await;
        let after = self.sink.usage().await;
        let memo_after = self.sink.memo_hits().await;
        self.sink.set_statement(None).await;
        // What this statement observed feeds the next session's estimates.
        if let Err(e) = self.sink.persist_estimates().await {
            tracing::warn!(error = %e, "could not record estimates");
        }
        if let Some(t) = &self.tracer {
            t.emit(TraceEvent::StatementFinished {
                statement: id,
                rows: rendered.rows,
                usage: BudgetUsage {
                    calls: after.calls - before.calls,
                    tokens: after.tokens - before.tokens,
                    dollars: after.dollars - before.dollars,
                    wall: started.elapsed(),
                },
                error: rendered.error.clone(),
                elapsed: started.elapsed(),
            });
            if rendered.is_final {
                t.emit(TraceEvent::Final {
                    session: self.session,
                    rows: rendered.rows,
                });
            }
        }
        let mut text = rendered.text;
        let footer = call_footer(&before, &after, memo_after - memo_before);
        if !footer.is_empty() {
            text.push('\n');
            text.push_str(&footer);
        }
        Rendered {
            text,
            is_error: rendered.error.is_some(),
            is_final: rendered.is_final,
            rows: rendered.rows_batch,
        }
    }

    async fn dispatch(&self, stmt: &Statement, id: StatementId) -> Outcome {
        match &stmt.kind {
            StatementKind::CreateFunction { .. } => match self.sink.define_function(stmt).await {
                Ok(m) => Outcome::ok(m),
                Err(e) => Outcome::err(e.to_string()),
            },
            StatementKind::CreateAgent { .. } => match self.sink.define_agent(stmt).await {
                Ok(m) => Outcome::ok(m),
                Err(e) => Outcome::err(e.to_string()),
            },
            StatementKind::Set { key, value } => match self.sink.apply_set(key, value).await {
                Ok(m) => Outcome::ok(m),
                Err(e) => Outcome::err(e.to_string()),
            },
            StatementKind::Calibrate {
                function,
                sample,
                recall,
                precision,
                input,
            } => {
                // The sample: the first `sample` rows of the query, planned
                // through the algebra like any other statement.
                let sampled = Statement {
                    sql: stmt.sql.clone(),
                    kind: StatementKind::Query {
                        plan: kleene_sql::LogicalPlan::Limit {
                            input: Box::new(input.clone()),
                            offset: 0,
                            limit: Some(*sample),
                        },
                    },
                };
                let (sampled, _) = self.rewrite(&sampled).await;
                let mut ctx = ExecContext::new(self.sink.clone());
                ctx.max_recursion_rounds = self.sink.settings().await.max_recursion_rounds;
                let rows = match execute_statement(&sampled, ctx).await {
                    Ok(StatementResult::Rows(b)) => b.rows,
                    Ok(_) => return Outcome::err("CALIBRATE: the query returned no rows".into()),
                    Err(e) => return Outcome::err(format!("error: {e}")),
                };
                match self
                    .sink
                    .calibrate(function, &rows, *recall, *precision)
                    .await
                {
                    Ok(m) => Outcome::ok(m),
                    Err(e) => Outcome::err(e.to_string()),
                }
            }
            StatementKind::Explain { inner, analyze } => {
                let explained = match self.explain(inner).await {
                    Ok(t) => t,
                    Err(e) => return Outcome::err(e),
                };
                if let Some(t) = &self.tracer {
                    t.emit(TraceEvent::StatementPlanned {
                        statement: id,
                        explain: explained.clone(),
                        estimate: serde_json::json!({}),
                    });
                }
                if !analyze {
                    return Outcome::ok(explained);
                }
                let before = self.sink.usage().await;
                let memo_before = self.sink.memo_hits().await;
                let started = Instant::now();
                let inner_outcome = Box::pin(self.dispatch(inner, id)).await;
                let after = self.sink.usage().await;
                let memo_after = self.sink.memo_hits().await;
                let actual = format!(
                    "actual: {} rows, {}, {:.1?}{}",
                    inner_outcome.rows,
                    call_footer(&before, &after, memo_after - memo_before),
                    started.elapsed(),
                    inner_outcome
                        .error
                        .as_ref()
                        .map(|e| format!("\nerror: {e}"))
                        .unwrap_or_default()
                );
                Outcome {
                    text: format!("{explained}{actual}"),
                    rows: inner_outcome.rows,
                    error: inner_outcome.error,
                    is_final: false,
                    rows_batch: None,
                }
            }
            _ => {
                // Plan through the call algebra so rewrites (join order,
                // cascade, semi-join, cheap-first) apply, then refuse up front
                // what the remaining budget cannot pay for.
                let (stmt, planned) = self.rewrite(stmt).await;
                if let Some(cp) = &planned {
                    if matches!(stmt.kind, StatementKind::Final { .. }) && cp.root.starts_children()
                    {
                        return Outcome::err(format!(
                            "refused: FINAL would start child sessions (rlm/spawn) while answering, ~{:.0} calls and ~${:.4}, and a FINAL that fails or runs out of budget repeats all of it. Run the delegation first and keep its rows, then answer from the table:\n  CREATE TABLE results AS SELECT c.ordinal, r.answer FROM ctx c CROSS JOIN LATERAL rlm('...', c.text) r;\n  FINAL FROM (SELECT ordinal, answer FROM results ORDER BY ordinal);\n{}",
                            cp.total.calls,
                            cp.total.dollars,
                            kleene_algebra::explain(cp)
                        ));
                    }
                    let (budget, used) = self.sink.budget_state().await;
                    let remaining = budget.remaining(&used);
                    let over_calls = remaining
                        .calls
                        .is_some_and(|c| cp.total.calls > c as f64 + 0.5);
                    let over_dollars = remaining
                        .dollars
                        .is_some_and(|d| cp.total.dollars > d + 1e-9);
                    if over_calls || over_dollars {
                        return Outcome::err(format!(
                            "refused: this statement is estimated at ~{:.0} calls (~${:.4}) but the remaining budget is {} calls / {}; narrow the input (a filter, a LIMIT, a sample), use a cheaper alias, or ask for less\n{}",
                            cp.total.calls,
                            cp.total.dollars,
                            remaining
                                .calls
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "unlimited".into()),
                            remaining
                                .dollars
                                .map(|d| format!("${d:.4}"))
                                .unwrap_or_else(|| "unlimited $".into()),
                            kleene_algebra::explain(cp)
                        ));
                    }
                    // The estimate beside the actuals `StatementFinished`
                    // records: `bench plot` draws estimate accuracy and the
                    // plan space from these.
                    if let Some(t) = &self.tracer {
                        t.emit(TraceEvent::StatementPlanned {
                            statement: id,
                            explain: kleene_algebra::explain(cp),
                            estimate: serde_json::json!({
                                "rows": cp.total.rows,
                                "calls": cp.total.calls,
                                "tokens": cp.total.tokens,
                                "dollars": cp.total.dollars,
                                "depth": cp.total.depth,
                                "relations": cp.plan_space.as_ref().map(|p| p.relations),
                                "join_orders": cp.plan_space.as_ref().map(|p| p.orders),
                            }),
                        });
                    }
                }
                let mut ctx = ExecContext::new(self.sink.clone());
                ctx.max_recursion_rounds = self.sink.settings().await.max_recursion_rounds;
                match execute_statement(&stmt, ctx).await {
                    Ok(StatementResult::Rows(b)) => Outcome {
                        text: format!(
                            "{}{} row{}",
                            b.render_table(self.render.max_rows),
                            b.len(),
                            if b.len() == 1 { "" } else { "s" }
                        ),
                        rows: b.len() as u64,
                        error: None,
                        is_final: false,
                        rows_batch: None,
                    },
                    Ok(StatementResult::Final(b)) => Outcome {
                        text: format!("FINAL\n{}", b.render_table(self.render.max_rows)),
                        rows: b.len() as u64,
                        error: None,
                        is_final: true,
                        rows_batch: Some(b),
                    },
                    Ok(StatementResult::Affected { rows, message }) => Outcome {
                        text: message,
                        rows,
                        error: None,
                        is_final: false,
                        rows_batch: None,
                    },
                    Ok(StatementResult::Set { key, value }) => {
                        Outcome::ok(format!("set {key} = {value}"))
                    }
                    Err(e) => Outcome::err(format!("error: {e}")),
                }
            }
        }
    }

    /// Apply the call-algebra rewrites to a statement's query plan, using the
    /// store's row counts and this session's sampled selectivities. Returns
    /// the rewritten statement and its plan (for statements that have one).
    async fn rewrite(&self, stmt: &Statement) -> (Statement, Option<kleene_algebra::CallPlan>) {
        let catalog = self.sink.catalog().read().await.clone();
        let cost = self.cost_model().await;
        let mut s = stmt.clone();
        let mut planned = None;
        match &mut s.kind {
            StatementKind::Query { plan }
            | StatementKind::Final { plan }
            | StatementKind::CreateTableAs { plan, .. }
            | StatementKind::Insert { plan, .. } => {
                let cp = plan_calls(plan, &catalog, &cost);
                *plan = cp.logical();
                planned = Some(cp);
            }
            _ => {}
        }
        (s, planned)
    }
}

struct Outcome {
    text: String,
    rows: u64,
    error: Option<String>,
    is_final: bool,
    rows_batch: Option<kleene_core::Batch>,
}

impl Outcome {
    fn ok(text: String) -> Self {
        Self {
            text,
            rows: 0,
            error: None,
            is_final: false,
            rows_batch: None,
        }
    }
    fn err(text: String) -> Self {
        Self {
            text: text.clone(),
            rows: 0,
            error: Some(text),
            is_final: false,
            rows_batch: None,
        }
    }
}

/// Whether a statement is a CallSQL addition the SQL parser does not know.
fn is_extension(stmt: &str) -> bool {
    let head: String = stmt
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    head.starts_with("FINAL")
        || head.starts_with("CREATE FUNCTION")
        || head.starts_with("CREATE OR REPLACE FUNCTION")
        || head.starts_with("CREATE AGENT")
        || head.starts_with("CALL ")
        || head == "CALL"
        || head.starts_with("CALIBRATE ")
        || head == "CALIBRATE"
}

/// Split a script on `;` outside quotes, `$$...$$` bodies and comments;
/// trimmed, non-empty pieces without their terminator.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut chars = sql.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\'' | '$' => {
                // A quoted string or dollar-quoted body is copied whole; an
                // unterminated one runs to the end of the script.
                let len = kleene_sql::string_len(&sql[at..]).unwrap_or(sql.len() - at);
                let end = at + len;
                cur.push_str(&sql[at..end]);
                // `len` is in bytes and the iterator steps by char: skip
                // until it has passed the literal, or a multi-byte literal
                // (Japanese, accents, emoji) eats the SQL that follows it.
                while chars.peek().is_some_and(|(i, _)| *i < end) {
                    chars.next();
                }
            }
            '"' => {
                cur.push(c);
                while let Some((_, d)) = chars.next() {
                    cur.push(d);
                    if d == c {
                        if chars.peek().map(|p| p.1) == Some(c) {
                            cur.push(chars.next().map(|p| p.1).unwrap_or(c));
                        } else {
                            break;
                        }
                    }
                }
            }
            '-' if chars.peek().map(|p| p.1) == Some('-') => {
                for (_, d) in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
                cur.push('\n');
            }
            ';' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `N calls (M from memo), T tokens, $D` for a statement's slice of usage;
/// empty when no model call happened.
fn call_footer(before: &BudgetUsage, after: &BudgetUsage, memo_hits: u64) -> String {
    let paid = after.calls - before.calls;
    let calls = paid + memo_hits;
    if calls == 0 {
        return String::new();
    }
    let memo = if memo_hits > 0 {
        format!(" ({memo_hits} from memo)")
    } else {
        String::new()
    };
    format!(
        "{calls} call{}{memo}, {} tokens, ${:.4}",
        if calls == 1 { "" } else { "s" },
        after.tokens - before.tokens,
        after.dollars - before.dollars
    )
}

fn kind_name(k: &StatementKind) -> &'static str {
    match k {
        StatementKind::Query { .. } => "SELECT",
        StatementKind::CreateTable { .. } => "CREATE TABLE",
        StatementKind::CreateTableAs { .. } => "CREATE TABLE AS",
        StatementKind::Insert { .. } => "INSERT",
        StatementKind::DropTable { .. } => "DROP TABLE",
        StatementKind::CreateFunction { .. } => "CREATE FUNCTION",
        StatementKind::CreateAgent { .. } => "CREATE AGENT",
        StatementKind::Call { .. } => "CALL",
        StatementKind::Explain { .. } => "EXPLAIN",
        StatementKind::Set { .. } => "SET",
        StatementKind::Final { .. } => "FINAL",
        StatementKind::Calibrate { .. } => "CALIBRATE",
    }
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn scripts_split_outside_strings_and_dollar_quotes() {
        let script = "CALL write_file('a.py', $$import sys; print('x;y')\nsys.exit(main())\n$$);\n-- note; here\nSELECT 'it''s; fine' AS s; FINAL(1)";
        let pieces = split_statements(script);
        assert_eq!(pieces.len(), 3, "{pieces:?}");
        assert!(pieces[0].ends_with("$$)"), "{}", pieces[0]);
        assert_eq!(pieces[1], "SELECT 'it''s; fine' AS s");
        assert_eq!(pieces[2], "FINAL(1)");
        // A dollar sign that opens no quote is ordinary text.
        assert_eq!(split_statements("SELECT 1 AS $x; SELECT 2").len(), 2);
    }

    #[test]
    fn multi_byte_literals_keep_the_sql_after_them() {
        // The literal's length is in bytes; the text after it must survive
        // whole (a regression: the splitter once skipped that many chars).
        let script = "SELECT 'ループエンジニアリング' AS a, text FROM ctx WHERE text LIKE '%予算%'; SELECT 'é' AS e, 2 AS n; CALL write_file('x.txt', $$日本語; テキスト$$)";
        let pieces = split_statements(script);
        assert_eq!(
            pieces,
            [
                "SELECT 'ループエンジニアリング' AS a, text FROM ctx WHERE text LIKE '%予算%'",
                "SELECT 'é' AS e, 2 AS n",
                "CALL write_file('x.txt', $$日本語; テキスト$$)",
            ]
        );
        let long = "あ".repeat(900);
        let script =
            format!("INSERT INTO t SELECT '{long}' AS text UNION ALL SELECT 'b'; SELECT 1");
        let pieces = split_statements(&script);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        assert!(pieces[0].ends_with("UNION ALL SELECT 'b'"), "{}", pieces[0]);
    }
}
