//! One CallSQL statement at a time against a store: the core of the REPL and,
//! from M3, of the model's turn loop.

use crate::live::LiveSink;
use crate::sink::StoreSink;
use crate::RenderOptions;
use callgebra_algebra::{plan as plan_calls, CostModel};
use callgebra_core::{BudgetUsage, Catalog, SessionId, StatementId};
use callgebra_exec::{execute_statement, ExecContext, StatementResult};
use callgebra_llm::Provider;
use callgebra_sql::{parse, plan, plan_sql, render_error, Statement, StatementKind};
use callgebra_store::{DuckDbStore, Store};
use callgebra_tools::{catalog_entries, standard_tools, ToolContext};
use callgebra_trace::{TraceEvent, Tracer};
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
    Planned(Statement),
    Ast(Box<sqlparser::ast::Statement>, String),
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
    pub rows: Option<callgebra_core::Batch>,
}

/// What a REPL is built from.
pub struct ReplConfig {
    /// Workspace root for tools.
    pub workspace: PathBuf,
    /// Model provider, if any.
    pub provider: Option<Arc<dyn Provider>>,
    /// Trace sink, if any (the store's own sink is the usual choice).
    pub tracer: Option<Tracer>,
}

impl Repl {
    /// Open a REPL over a store with the standard catalog, no provider and
    /// tools rooted at the current directory.
    pub async fn new(store: DuckDbStore) -> Result<Self, callgebra_store::StoreError> {
        Self::with_config(
            store,
            ReplConfig {
                workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                provider: None,
                tracer: None,
            },
        )
        .await
    }

    /// Open a REPL with a provider, a workspace and a tracer.
    pub async fn with_config(
        store: DuckDbStore,
        cfg: ReplConfig,
    ) -> Result<Self, callgebra_store::StoreError> {
        let store_sink = StoreSink::new(store, callgebra_core::standard_catalog()).await?;
        let tools = Arc::new(standard_tools());
        let entries = catalog_entries(&tools);
        let live = LiveSink::new(
            store_sink,
            cfg.provider,
            tools,
            ToolContext::new(cfg.workspace),
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

    /// The live catalog.
    pub fn catalog(&self) -> Arc<tokio::sync::RwLock<Catalog>> {
        self.sink.catalog()
    }

    /// This REPL's session id (trace attribution).
    pub fn session(&self) -> SessionId {
        self.session
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
        Ok(callgebra_algebra::explain(&cp))
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
                // CallSQL additions the parser does not know are planned now,
                // one statement at a time.
                let catalog = self.sink.catalog().read().await.clone();
                match plan_sql(&piece, &catalog) {
                    Ok(stmts) => pending.extend(stmts.into_iter().map(Pending::Planned)),
                    Err(e) => {
                        let mut out = self.run_pending(pending).await;
                        if out.last().is_none_or(|r| !r.is_error && !r.is_final) {
                            out.push(self.failed_to_plan(&piece, &e));
                        }
                        return out;
                    }
                }
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
        for item in pending {
            let stmt = match item {
                Pending::Planned(s) => s,
                Pending::Ast(ast, text) => {
                    let catalog = self.sink.catalog().read().await.clone();
                    match plan(&ast, &catalog) {
                        Ok(mut s) => {
                            s.sql = text;
                            s
                        }
                        Err(e) => {
                            out.push(self.failed_to_plan(&text, &e));
                            break;
                        }
                    }
                }
            };
            let rendered = self.run_one(&stmt).await;
            let stop = rendered.is_error || rendered.is_final;
            out.push(rendered);
            if stop {
                break;
            }
        }
        out
    }

    /// Render a parse or planning failure, and trace it like any other failed
    /// statement so the transcript in `trace_statements` is complete.
    fn failed_to_plan(&self, sql: &str, e: &callgebra_sql::SqlError) -> Rendered {
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
        self.sink.set_statement(Some(id)).await;
        let before = self.sink.usage().await;
        let memo_before = self.sink.memo_hits().await;
        let rendered = self.dispatch(stmt, id).await;
        let after = self.sink.usage().await;
        let memo_after = self.sink.memo_hits().await;
        self.sink.set_statement(None).await;
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
                // Plan through the call algebra so rewrites (semi-join, cheap-first) apply.
                let stmt = self.rewrite(stmt).await;
                let ctx = ExecContext::new(self.sink.clone());
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

    /// Apply the call-algebra rewrites to a statement's query plan.
    async fn rewrite(&self, stmt: &Statement) -> Statement {
        let catalog = self.sink.catalog().read().await.clone();
        let cost = CostModel::default();
        let mut s = stmt.clone();
        match &mut s.kind {
            StatementKind::Query { plan }
            | StatementKind::Final { plan }
            | StatementKind::CreateTableAs { plan, .. }
            | StatementKind::Insert { plan, .. } => {
                let cp = plan_calls(plan, &catalog, &cost);
                *plan = cp.logical();
            }
            _ => {}
        }
        s
    }
}

struct Outcome {
    text: String,
    rows: u64,
    error: Option<String>,
    is_final: bool,
    rows_batch: Option<callgebra_core::Batch>,
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
}

/// Split a script on `;` outside quotes and comments; trimmed, non-empty
/// pieces without their terminator.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                cur.push(c);
                while let Some(d) = chars.next() {
                    cur.push(d);
                    if d == c {
                        if chars.peek() == Some(&c) {
                            cur.push(chars.next().unwrap_or(c));
                        } else {
                            break;
                        }
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                let comment: String = chars.by_ref().take_while(|&d| d != '\n').collect();
                let _ = comment;
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
        StatementKind::CreateTableAs { .. } => "CREATE TABLE AS",
        StatementKind::Insert { .. } => "INSERT",
        StatementKind::DropTable { .. } => "DROP TABLE",
        StatementKind::CreateFunction { .. } => "CREATE FUNCTION",
        StatementKind::CreateAgent { .. } => "CREATE AGENT",
        StatementKind::Call { .. } => "CALL",
        StatementKind::Explain { .. } => "EXPLAIN",
        StatementKind::Set { .. } => "SET",
        StatementKind::Final { .. } => "FINAL",
    }
}
