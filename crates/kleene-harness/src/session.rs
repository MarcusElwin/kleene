//! Sessions: the model's SQL turn loop, child sessions for `rlm(...)` and
//! `spawn(...)`, persistence and resume.
//!
//! A [`Harness`] owns a store, a provider and the tools. [`Harness::run`]
//! starts a root session for a task; each turn sends the cached system prefix
//! plus the transcript, extracts the SQL from the reply, runs it through the
//! [`Repl`], renders the results back, and stops at `FINAL`, an exhausted
//! budget, the turn cap or an unrecoverable error. Children are the same loop
//! at depth + 1 with a role, a budget slice and their own table namespace.

use crate::live::{ChildRunner, DefinedFunction, LiveSink, ModelSettings, SessionMeta};
use crate::prompt::{self, ContextSummary, PromptContext};
use crate::repl::{Rendered, Repl};
use crate::sink::StoreSink;
use crate::{AgentRole, Outcome, RenderOptions};
use kleene_core::{
    Batch, Budget, BudgetUsage, CallId, CallKind, Catalog, DataType, Field, FunctionDef, RunId,
    Schema, SessionId, StatementId, TableSource, Value,
};
use kleene_exec::ExecError;
use kleene_llm::{CompletionRequest, Message, Provider, ProviderOptions, StopReason};
use kleene_store::duckdb::literal;
use kleene_store::{DuckDbStore, StoreError};
use kleene_tools::{catalog_entries, standard_tools, ToolContext, ToolRegistry, WebSearchBackend};
use kleene_trace::{TraceEvent, Tracer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Harness errors.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Execution failed outside a statement (setting up `ctx`, for instance).
    #[error(transparent)]
    Exec(#[from] ExecError),
    /// No such run, or nothing to resume in it.
    #[error("cannot resume run {run}: {reason}")]
    Resume {
        /// The run.
        run: String,
        /// Why.
        reason: String,
    },
    /// A persisted record could not be decoded.
    #[error("corrupt session record: {0}")]
    Corrupt(String),
    /// A configuration or request problem (unknown generator, no provider).
    #[error("{0}")]
    Config(String),
}

/// How a harness runs sessions.
#[derive(Clone)]
pub struct HarnessConfig {
    /// Workspace root for tools.
    pub workspace: PathBuf,
    /// Model provider (`None` makes every model call fail with a clear error).
    pub provider: Option<Arc<dyn Provider>>,
    /// Trace sink.
    pub tracer: Option<Tracer>,
    /// Deepest session allowed; `rlm`/`spawn` are refused beyond it.
    pub max_depth: u32,
    /// Turn cap for the root session.
    pub max_turns: u32,
    /// Turn cap for children whose role does not set one.
    pub child_max_turns: u32,
    /// Fraction of the parent's remaining budget a child may spend.
    pub child_budget_fraction: f64,
    /// The root session's budget.
    pub budget: Budget,
    /// How results are rendered for the model.
    pub render: RenderOptions,
    /// Output cap for one model turn.
    pub max_tokens: u32,
    /// Who watches turns as they happen (the CLI, later the daemon).
    pub observer: Option<Arc<dyn Observer>>,
    /// Learned playbook entries rendered into the prompt as examples.
    pub playbook: Vec<crate::PlaybookExample>,
    /// Skip the memo so every call is real (replay evals compare true cost).
    pub no_memo: bool,
    /// The service behind `web_search`; `None` makes every search fail.
    pub web_search: Option<WebSearchBackend>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            provider: None,
            tracer: None,
            max_depth: 2,
            max_turns: 30,
            child_max_turns: 12,
            child_budget_fraction: 0.5,
            budget: Budget::unbounded(),
            render: RenderOptions::default(),
            max_tokens: 4096,
            observer: None,
            playbook: vec![],
            no_memo: false,
            web_search: None,
        }
    }
}

/// Watches sessions as they run. Every hook has a no-op default; the
/// call hooks fire from inside [`LiveSink`], so an observer sees the
/// session's own reply stream in ([`Observer::call_delta`]) before
/// [`Observer::turn`] reports what its SQL did.
pub trait Observer: Send + Sync {
    /// A session began.
    fn session_started(&self, _meta: &SessionMeta, _task: &str) {}
    /// A model call began. `turn` is true for the session's own reply and
    /// false for a call made by a statement (`llm(...)`, `rlm(...)`, …).
    fn call_started(&self, _meta: &SessionMeta, _call: CallId, _alias: &str, _turn: bool) {}
    /// A piece of the call's text as it is generated. Only the session's own
    /// reply streams; a memo hit delivers the whole text in one piece.
    fn call_delta(&self, _meta: &SessionMeta, _call: CallId, _text: &str) {}
    /// A model call ended, served from the memo or not, with an error if it
    /// failed.
    fn call_finished(
        &self,
        _meta: &SessionMeta,
        _call: CallId,
        _memo_hit: bool,
        _error: Option<&str>,
    ) {
    }
    /// A turn completed (the model replied and its SQL ran).
    fn turn(&self, _meta: &SessionMeta, _turn: &Turn) {}
    /// A session ended.
    fn session_finished(&self, _meta: &SessionMeta, _outcome: &Outcome, _usage: &BudgetUsage) {}
}

/// One model turn and what came of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// 1-based turn number within the session.
    pub n: u32,
    /// The model's reply, verbatim.
    pub reply: String,
    /// The SQL extracted from it, if any.
    pub sql: Option<String>,
    /// What each statement rendered to.
    pub results: Vec<Rendered>,
    /// The user message sent back.
    pub feedback: String,
}

/// Where a session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Still taking turns (or interrupted mid-turn).
    Running,
    /// Ended.
    Finished,
}

/// What a session did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReport {
    /// Session.
    pub session: SessionId,
    /// Run.
    pub run: RunId,
    /// Depth.
    pub depth: u32,
    /// Role name.
    pub role: String,
    /// Task text.
    pub task: String,
    /// How it ended.
    pub outcome: Outcome,
    /// Turns taken (including resumed ones).
    pub turns: u32,
    /// Spending, children included.
    pub usage: BudgetUsage,
    /// The turns of this invocation (a resumed session lists only the new ones).
    pub transcript: Vec<Turn>,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// Run.
    pub run: RunId,
    /// The root session.
    pub root: SessionReport,
}

/// The persisted part of a session, enough to resume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecord {
    role: AgentRole,
    context: Option<String>,
    budget: Budget,
    usage: BudgetUsage,
    agents: HashMap<String, AgentRole>,
    functions: HashMap<String, DefinedFunction>,
    settings: ModelSettings,
    messages: Vec<Message>,
    turns: u32,
    max_turns: u32,
}

/// Everything a session starts from.
struct SessionSpec {
    meta: SessionMeta,
    parent: Option<SessionId>,
    task: String,
    record: SessionRecord,
    base_catalog: Catalog,
}

/// Runs sessions over one store.
pub struct Harness {
    store: DuckDbStore,
    cfg: HarnessConfig,
    tools: Arc<ToolRegistry>,
    tool_entries: Vec<FunctionDef>,
    /// Live sessions, for cancellation and inspection.
    live: std::sync::Mutex<HashMap<SessionId, Arc<LiveSink>>>,
}

const SESSIONS_DDL: &str = "CREATE TABLE IF NOT EXISTS kleene_sessions (session VARCHAR PRIMARY KEY, run VARCHAR, parent VARCHAR, depth INTEGER, role VARCHAR, task VARCHAR, status VARCHAR, outcome VARCHAR, turns INTEGER, record VARCHAR, updated_at TIMESTAMP)";

impl Harness {
    /// Open a harness over a store.
    pub async fn new(store: DuckDbStore, cfg: HarnessConfig) -> Result<Arc<Self>, HarnessError> {
        store.execute(SESSIONS_DDL).await?;
        let tools = Arc::new(standard_tools());
        let tool_entries = catalog_entries(&tools);
        Ok(Arc::new(Self {
            store,
            cfg,
            tools,
            tool_entries,
            live: std::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// Sessions currently running.
    pub fn live_sessions(&self) -> Vec<SessionId> {
        self.live
            .lock()
            .map(|l| l.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Stop the statement a live session is running at its next call; the
    /// session itself continues with the error rendered back to the model.
    /// Returns whether the session was live.
    pub fn cancel_statement(&self, session: SessionId) -> bool {
        match self.live.lock().ok().and_then(|l| l.get(&session).cloned()) {
            Some(sink) => {
                sink.cancel_statement();
                true
            }
            None => false,
        }
    }

    fn register(&self, id: SessionId, sink: &Arc<LiveSink>) {
        if let Ok(mut l) = self.live.lock() {
            l.insert(id, sink.clone());
        }
    }

    fn unregister(&self, id: SessionId) {
        if let Ok(mut l) = self.live.lock() {
            l.remove(&id);
        }
    }

    /// The store.
    pub fn store(&self) -> &DuckDbStore {
        &self.store
    }

    /// The configuration.
    pub fn config(&self) -> &HarnessConfig {
        &self.cfg
    }

    /// The root catalog: standard functions, tools, and whatever tables the
    /// store holds (added by the sink on refresh).
    fn root_catalog(&self) -> Catalog {
        let mut c = kleene_core::standard_catalog();
        for e in &self.tool_entries {
            c.add_function(e.clone());
        }
        c
    }

    /// Run a task to completion in a fresh root session.
    pub async fn run(
        self: &Arc<Self>,
        task: &str,
        context: Option<String>,
    ) -> Result<RunReport, HarnessError> {
        self.run_as(RunId::new(), task, context).await
    }

    /// [`Harness::run`] under a caller-chosen run id (the daemon announces the
    /// id before the run starts).
    pub async fn run_as(
        self: &Arc<Self>,
        run: RunId,
        task: &str,
        context: Option<String>,
    ) -> Result<RunReport, HarnessError> {
        if let Some(t) = &self.cfg.tracer {
            t.emit(TraceEvent::RunStarted {
                run,
                task: task.to_string(),
            });
        }
        let role = AgentRole::root();
        let meta = SessionMeta {
            id: SessionId::new(),
            run,
            depth: 0,
            role: role.clone(),
        };
        let record = SessionRecord {
            role,
            context,
            budget: self.cfg.budget.clone(),
            usage: BudgetUsage::default(),
            agents: HashMap::new(),
            functions: HashMap::new(),
            settings: ModelSettings {
                no_memo: self.cfg.no_memo,
                ..ModelSettings::default()
            },
            messages: vec![],
            turns: 0,
            max_turns: self.cfg.max_turns,
        };
        let spec = SessionSpec {
            meta,
            parent: None,
            task: task.to_string(),
            record,
            base_catalog: self.root_catalog(),
        };
        let root = self.run_session(spec).await?;
        Ok(RunReport { run, root })
    }

    /// Continue a run whose root session did not reach `FINAL` (turn cap, or
    /// interrupted), granting it `config.max_turns` more turns.
    pub async fn resume(self: &Arc<Self>, run: RunId) -> Result<RunReport, HarnessError> {
        let sql = format!(
            "SELECT session, task, status, outcome, record FROM kleene_sessions WHERE run = {} AND parent IS NULL",
            literal(&Value::Text(run.to_string()))
        );
        let rows = self.store.query(&sql).await?;
        let Some(row) = rows.rows.first() else {
            return Err(HarnessError::Resume {
                run: run.to_string(),
                reason: "no root session recorded".into(),
            });
        };
        let session: SessionId = row[0]
            .as_text()
            .and_then(|s| s.parse().ok())
            .map(SessionId)
            .ok_or_else(|| HarnessError::Corrupt("session id".into()))?;
        let task = row[1].as_text().unwrap_or_default().to_string();
        let outcome = row[3].as_text().unwrap_or_default();
        if !matches!(outcome, "" | "turns_exhausted" | "running") {
            return Err(HarnessError::Resume {
                run: run.to_string(),
                reason: format!("its root session ended with {outcome}"),
            });
        }
        let record: SessionRecord = serde_json::from_str(row[4].as_text().unwrap_or_default())
            .map_err(|e| HarnessError::Corrupt(e.to_string()))?;
        let mut record = record;
        record.max_turns = record.turns + self.cfg.max_turns;
        let mut base = self.root_catalog();
        for (name, def) in &record.functions {
            base.add_function(crate::live::function_entry(name, def));
        }
        let meta = SessionMeta {
            id: session,
            run,
            depth: 0,
            role: record.role.clone(),
        };
        let spec = SessionSpec {
            meta,
            parent: None,
            task,
            record,
            base_catalog: base,
        };
        let root = self.run_session(spec).await?;
        Ok(RunReport { run, root })
    }

    async fn run_session(
        self: &Arc<Self>,
        spec: SessionSpec,
    ) -> Result<SessionReport, HarnessError> {
        let SessionSpec {
            meta,
            parent,
            task,
            mut record,
            base_catalog,
        } = spec;
        let namespace = (meta.depth > 0).then(|| short_id(meta.id));
        let store_sink =
            StoreSink::with_namespace(self.store.clone(), base_catalog, namespace).await?;
        let sink = Arc::new(LiveSink::new(
            store_sink,
            self.cfg.provider.clone(),
            self.tools.clone(),
            ToolContext::new(self.cfg.workspace.clone())
                .with_web_search(self.cfg.web_search.clone()),
            self.cfg.tracer.clone(),
        ));
        sink.set_meta(meta.clone()).await;
        sink.set_observer(self.cfg.observer.clone()).await;
        sink.set_agents(record.agents.clone()).await;
        sink.set_functions(record.functions.clone()).await;
        sink.set_budget(record.budget.clone()).await;
        let mut settings = record.settings.clone();
        if settings.effort.is_none() {
            settings.effort = record.role.effort.clone();
        }
        sink.set_settings(settings).await;
        sink.set_child_runner(Arc::new(self.clone())).await;
        sink.charge_child(&record.usage).await;
        self.register(meta.id, &sink);
        let fresh = record.messages.is_empty();
        let mut context_summary = None;
        if let Some(ctx) = &record.context {
            if fresh {
                load_context(&sink, ctx).await?;
            }
            context_summary = Some(ContextSummary::of(ctx, paragraphs(ctx).len() as u64));
        }
        let repl = Repl::from_sink(
            sink.clone(),
            meta.id,
            self.cfg.tracer.clone(),
            self.cfg.render.clone(),
        );
        if let Some(t) = &self.cfg.tracer {
            t.emit(TraceEvent::SessionStarted {
                run: meta.run,
                session: meta.id,
                parent,
                depth: meta.depth,
                role: record.role.name.clone(),
                task: task.clone(),
            });
        }
        if let Some(o) = &self.cfg.observer {
            o.session_started(&meta, &task);
        }
        let agents: Vec<AgentRole> = {
            let mut a: Vec<AgentRole> = record.agents.values().cloned().collect();
            a.sort_by(|x, y| x.name.cmp(&y.name));
            a
        };
        let catalog = sink.catalog().read().await.clone();
        let system = prompt::system_prompt(&PromptContext {
            catalog: &catalog,
            role: &record.role,
            agents: &agents,
            budget: &record.budget,
            depth: meta.depth,
            max_depth: self.cfg.max_depth,
            max_turns: record.max_turns,
            playbook: &self.cfg.playbook,
        });
        if fresh {
            record.messages.push(Message::user(prompt::task_message(
                &task,
                context_summary.as_ref(),
            )));
        }
        self.persist(&meta, parent, &task, SessionStatus::Running, None, &record)
            .await?;

        let started = Instant::now();
        let mut transcript = vec![];
        let outcome = loop {
            if record.turns >= record.max_turns {
                break Outcome::TurnsExhausted;
            }
            record.turns += 1;
            let turn_no = record.turns;
            // The turn itself is traced as a pseudo-statement so its call
            // shows up under the session.
            let turn_stmt = StatementId::new();
            if let Some(t) = &self.cfg.tracer {
                t.emit(TraceEvent::StatementStarted {
                    session: meta.id,
                    statement: turn_stmt,
                    sql: format!("-- turn {turn_no}"),
                });
            }
            sink.set_statement(Some(turn_stmt)).await;
            let before = sink.usage().await;
            let turn_started = Instant::now();
            let req = CompletionRequest {
                alias: record.role.model.clone(),
                model: String::new(),
                system: system.clone(),
                messages: record.messages.clone(),
                tools: vec![],
                output_schema: None,
                max_tokens: self.cfg.max_tokens,
                options: ProviderOptions {
                    effort: record.role.effort.clone(),
                    cache_prefix: true,
                    temperature: None,
                    extra: Default::default(),
                },
            };
            let resp = sink.complete_streaming(req).await;
            let after = sink.usage().await;
            sink.set_statement(None).await;
            if let Some(t) = &self.cfg.tracer {
                t.emit(TraceEvent::StatementFinished {
                    statement: turn_stmt,
                    rows: 0,
                    usage: diff(&before, &after, turn_started.elapsed()),
                    error: resp.as_ref().err().map(|e| e.to_string()),
                    elapsed: turn_started.elapsed(),
                });
            }
            let resp = match resp {
                Ok(r) => r,
                Err(ExecError::Budget(b)) => {
                    break Outcome::BudgetExhausted {
                        detail: b.to_string(),
                    }
                }
                Err(e) => {
                    break Outcome::Failed {
                        error: e.to_string(),
                    }
                }
            };
            let mut reply = resp.text();
            if resp.stop_reason == StopReason::Refusal {
                break Outcome::Failed {
                    error: "the model declined the task".into(),
                };
            }
            if reply.trim().is_empty() {
                reply = "(empty reply)".into();
            }
            record.messages.push(Message::assistant(reply.clone()));
            let sql = prompt::extract_sql(&reply);
            let (results, feedback) = match &sql {
                None => {
                    let reason = if resp.stop_reason == StopReason::MaxTokens {
                        "Your reply was cut off before any complete SQL."
                    } else {
                        "No SQL found in your reply."
                    };
                    (vec![], prompt::nudge(reason))
                }
                Some(sql) => {
                    let results = repl.submit(sql).await;
                    let (budget, used) = sink.budget_state().await;
                    let feedback = prompt::render_results(
                        &results,
                        turn_no,
                        record.max_turns,
                        &used,
                        &budget.remaining(&used),
                    );
                    (results, feedback)
                }
            };
            let turn = Turn {
                n: turn_no,
                reply,
                sql,
                results,
                feedback: feedback.clone(),
            };
            let final_rows = turn
                .results
                .iter()
                .find(|r| r.is_final)
                .and_then(|r| r.rows.clone());
            let budget_hit = turn
                .results
                .iter()
                .find(|r| r.is_error && is_budget_error(&r.text))
                .map(|r| r.text.clone());
            record.messages.push(Message::user(feedback));
            if let Some(o) = &self.cfg.observer {
                o.turn(&meta, &turn);
            }
            transcript.push(turn);
            record.usage = sink.usage().await;
            self.persist(&meta, parent, &task, SessionStatus::Running, None, &record)
                .await?;
            if let Some(answer) = final_rows {
                break Outcome::Final { answer };
            }
            if let Some(detail) = budget_hit {
                break Outcome::BudgetExhausted { detail };
            }
        };
        self.unregister(meta.id);
        let mut usage = sink.usage().await;
        usage.wall = started.elapsed();
        record.usage = usage;
        self.persist(
            &meta,
            parent,
            &task,
            SessionStatus::Finished,
            Some(&outcome),
            &record,
        )
        .await?;
        if let Some(t) = &self.cfg.tracer {
            t.emit(TraceEvent::SessionFinished {
                session: meta.id,
                outcome: outcome.tag().into(),
                turns: record.turns,
                usage,
            });
        }
        if let Some(o) = &self.cfg.observer {
            o.session_finished(&meta, &outcome, &usage);
        }
        Ok(SessionReport {
            session: meta.id,
            run: meta.run,
            depth: meta.depth,
            role: record.role.name.clone(),
            task,
            outcome,
            turns: record.turns,
            usage,
            transcript,
        })
    }

    async fn persist(
        &self,
        meta: &SessionMeta,
        parent: Option<SessionId>,
        task: &str,
        status: SessionStatus,
        outcome: Option<&Outcome>,
        record: &SessionRecord,
    ) -> Result<(), HarnessError> {
        let t = |s: &str| literal(&Value::Text(s.to_string()));
        let record_json = serde_json::to_string(record).unwrap_or_default();
        let sql = format!(
            "INSERT OR REPLACE INTO kleene_sessions VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, now())",
            t(&meta.id.to_string()),
            t(&meta.run.to_string()),
            parent.map(|p| t(&p.to_string())).unwrap_or_else(|| "NULL".into()),
            meta.depth,
            t(&record.role.name),
            t(task),
            t(match status {
                SessionStatus::Running => "running",
                SessionStatus::Finished => "finished",
            }),
            t(outcome.map(Outcome::tag).unwrap_or("running")),
            record.turns,
            t(&record_json),
        );
        self.store.execute(&sql).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ChildRunner for Arc<Harness> {
    async fn run_child(
        &self,
        parent: &LiveSink,
        agent: Option<&str>,
        task: String,
        context: Option<String>,
    ) -> Result<Batch, ExecError> {
        let pmeta = parent.meta().await;
        let depth = pmeta.depth + 1;
        if depth > self.cfg.max_depth {
            return Err(ExecError::Call(format!(
                "{} refused: depth {depth} exceeds the maximum {} (SET budget.depth, or answer here)",
                if agent.is_some() { "spawn" } else { "rlm" },
                self.cfg.max_depth
            )));
        }
        let agents = parent.agents().await;
        let role = match agent {
            None => AgentRole::child_of(&pmeta.role),
            Some(name) => agents
                .get(&name.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| {
                    ExecError::Call(format!(
                        "spawn: no agent named {name}; declare it with CREATE AGENT {name} ..."
                    ))
                })?,
        };
        let (pbudget, pused) = parent.budget_state().await;
        if let Some(limit) = pbudget.max_depth {
            if depth > limit {
                return Err(ExecError::Call(format!(
                    "depth {depth} exceeds this session's budget.depth {limit}"
                )));
            }
        }
        let remaining = pbudget.remaining(&pused);
        let budget = intersect(
            &remaining.slice(self.cfg.child_budget_fraction),
            &role.budget,
        );
        // A child that starts with nothing to spend fails on its first call
        // with a bare "call budget exceeded: 1 > 0". Refuse it here instead,
        // naming what the parent has left and how the slice is computed, so
        // the model can raise the budget or answer in this session.
        let what = if agent.is_some() { "spawn" } else { "rlm" };
        let pct = (self.cfg.child_budget_fraction * 100.0).round() as u64;
        if let (Some(0), Some(limit)) = (budget.calls, pbudget.calls) {
            return Err(ExecError::Call(format!(
                "{what} refused: a child gets {pct}% of this session's remaining calls, rounded down, and {} of budget.calls {limit} remain, so it would get none; SET budget.calls higher or answer here",
                remaining.calls.unwrap_or(0)
            )));
        }
        if let (Some(0), Some(limit)) = (budget.tokens, pbudget.tokens) {
            return Err(ExecError::Call(format!(
                "{what} refused: a child gets {pct}% of this session's remaining tokens, rounded down, and {} of budget.tokens {limit} remain, so it would get none; SET budget.tokens higher or answer here",
                remaining.tokens.unwrap_or(0)
            )));
        }
        // The child's view: the parent's functions, tools restricted to the
        // role's list, none of the parent's tables.
        let mut base = parent.catalog().read().await.clone();
        let stored: Vec<String> = base
            .tables()
            .filter(|t| t.source == TableSource::Stored)
            .map(|t| t.name.clone())
            .collect();
        for t in stored {
            base.remove_table(&t);
        }
        if !role.tools.is_empty() {
            let allowed: Vec<String> = role.tools.iter().map(|t| t.to_ascii_lowercase()).collect();
            let denied: Vec<String> = base
                .functions()
                .filter(|f| matches!(f.call_kind, CallKind::Tool { .. }))
                .filter(|f| !allowed.contains(&f.name.to_ascii_lowercase()))
                .map(|f| f.name.clone())
                .collect();
            for f in denied {
                base.remove_function(&f);
            }
        }
        let max_turns = role.max_turns.unwrap_or(self.cfg.child_max_turns);
        let meta = SessionMeta {
            id: SessionId::new(),
            run: pmeta.run,
            depth,
            role: role.clone(),
        };
        let record = SessionRecord {
            role,
            context,
            budget,
            usage: BudgetUsage::default(),
            agents,
            functions: parent.functions().await,
            settings: parent.settings().await,
            messages: vec![],
            turns: 0,
            max_turns,
        };
        let spec = SessionSpec {
            meta,
            parent: Some(pmeta.id),
            task,
            record,
            base_catalog: base,
        };
        let report = self
            .run_session(spec)
            .await
            .map_err(|e| ExecError::Call(e.to_string()))?;
        parent.charge_child(&report.usage).await;
        Ok(child_relation(&report, agent.is_some()))
    }
}

/// Rows of `rlm`/`spawn`: answer (first column), detail (the row as JSON) and,
/// for `spawn`, the child session id. A child that did not reach `FINAL`
/// yields one row with a NULL answer and the outcome in `detail`.
fn child_relation(report: &SessionReport, with_session: bool) -> Batch {
    let mut fields = vec![
        Field::new("answer", DataType::Text),
        Field::new("detail", DataType::Json),
    ];
    if with_session {
        fields.push(Field::new("session", DataType::Text));
    }
    let schema = Arc::new(Schema::new(fields));
    let session = Value::Text(report.session.to_string());
    let rows: Vec<Vec<Value>> = match &report.outcome {
        Outcome::Final { answer } => answer
            .rows
            .iter()
            .map(|r| {
                let mut obj = serde_json::Map::new();
                for (f, v) in answer.schema.fields.iter().zip(r) {
                    obj.insert(f.name.clone(), value_json(v));
                }
                let first = match r.first() {
                    Some(Value::Null) | None => Value::Null,
                    Some(v) => Value::Text(v.render()),
                };
                let mut row = vec![first, Value::Json(serde_json::Value::Object(obj))];
                if with_session {
                    row.push(session.clone());
                }
                row
            })
            .collect(),
        other => {
            let detail = serde_json::json!({
                "outcome": other.tag(),
                "error": match other {
                    Outcome::BudgetExhausted { detail } => detail.clone(),
                    Outcome::Failed { error } => error.clone(),
                    Outcome::TurnsExhausted => format!("no FINAL after {} turns", report.turns),
                    _ => String::new(),
                },
                "turns": report.turns,
                "calls": report.usage.calls,
            });
            let mut row = vec![Value::Null, Value::Json(detail)];
            if with_session {
                row.push(session.clone());
            }
            vec![row]
        }
    };
    Batch::try_new(schema.clone(), rows).unwrap_or_else(|_| Batch::empty(schema))
}

fn value_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(f.to_string())),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Json(j) => j.clone(),
        Value::Vector(v) => serde_json::Value::Array(
            v.iter()
                .map(|x| serde_json::Value::from(f64::from(*x)))
                .collect(),
        ),
    }
}

/// Split a context into paragraphs (blank-line separated, trimmed, non-empty);
/// a context without blank lines is one row.
pub fn paragraphs(text: &str) -> Vec<String> {
    let parts: Vec<String> = text
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        vec![text.trim().to_string()]
    } else {
        parts
    }
}

async fn load_context(sink: &LiveSink, ctx: &str) -> Result<(), ExecError> {
    use kleene_exec::CallSink;
    let schema = Arc::new(Schema::new(vec![
        Field::not_null("ordinal", DataType::Int),
        Field::new("text", DataType::Text),
    ]));
    // A fresh run replaces whatever an earlier run in this store left behind.
    sink.drop_table("ctx", true).await?;
    sink.create_table("ctx", schema.clone(), false).await?;
    let rows: Vec<Vec<Value>> = paragraphs(ctx)
        .into_iter()
        .enumerate()
        .map(|(i, p)| vec![Value::Int(i as i64), Value::Text(p)])
        .collect();
    let batch = Batch::try_new(schema, rows)?;
    sink.insert("ctx", batch).await?;
    sink.store()
        .refresh()
        .await
        .map_err(|e| ExecError::Eval(e.to_string()))?;
    Ok(())
}

/// The tighter of two budgets, dimension by dimension.
pub fn intersect(a: &Budget, b: &Budget) -> Budget {
    fn min_opt<T: PartialOrd + Copy>(x: Option<T>, y: Option<T>) -> Option<T> {
        match (x, y) {
            (Some(x), Some(y)) => Some(if x < y { x } else { y }),
            (x, None) => x,
            (None, y) => y,
        }
    }
    Budget {
        calls: min_opt(a.calls, b.calls),
        tokens: min_opt(a.tokens, b.tokens),
        dollars: min_opt(a.dollars, b.dollars),
        max_depth: min_opt(a.max_depth, b.max_depth),
        wall: min_opt(a.wall, b.wall),
    }
}

fn diff(before: &BudgetUsage, after: &BudgetUsage, wall: std::time::Duration) -> BudgetUsage {
    BudgetUsage {
        calls: after.calls.saturating_sub(before.calls),
        tokens: after.tokens.saturating_sub(before.tokens),
        dollars: (after.dollars - before.dollars).max(0.0),
        wall,
    }
}

fn is_budget_error(text: &str) -> bool {
    text.contains("budget exceeded") || text.contains("exceeds max depth")
}

/// Twelve hex characters of a session id, for table namespaces: the tail,
/// which is random, not the head, which is the creation time and is shared
/// by sessions spawned in the same millisecond.
pub fn short_id(id: SessionId) -> String {
    let s = id.0.simple().to_string();
    s[s.len() - 12..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_intersect_dimensionwise() {
        let a = Budget {
            calls: Some(10),
            tokens: None,
            dollars: Some(2.0),
            ..Budget::unbounded()
        };
        let b = Budget {
            calls: Some(4),
            tokens: Some(100),
            dollars: None,
            ..Budget::unbounded()
        };
        let c = intersect(&a, &b);
        assert_eq!(c.calls, Some(4));
        assert_eq!(c.tokens, Some(100));
        assert_eq!(c.dollars, Some(2.0));
    }

    #[test]
    fn short_ids_differ_for_ids_created_together() {
        let a = short_id(SessionId::new());
        let b = short_id(SessionId::new());
        assert_ne!(a, b);
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        assert_eq!(paragraphs("a\nb\n\nc\r\n\r\nd"), ["a\nb", "c", "d"]);
        assert_eq!(paragraphs("single"), ["single"]);
        assert_eq!(paragraphs("   "), [""]);
    }

    #[test]
    fn child_relation_shapes() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("verdict", DataType::Text),
            Field::new("n", DataType::Int),
        ]));
        let answer =
            Batch::try_new(schema, vec![vec![Value::Text("ok".into()), Value::Int(3)]]).unwrap();
        let report = SessionReport {
            session: SessionId::new(),
            run: RunId::new(),
            depth: 1,
            role: "self".into(),
            task: "t".into(),
            outcome: Outcome::Final { answer },
            turns: 2,
            usage: BudgetUsage::default(),
            transcript: vec![],
        };
        let b = child_relation(&report, true);
        assert_eq!(b.schema.names(), ["answer", "detail", "session"]);
        assert_eq!(b.rows[0][0], Value::Text("ok".into()));
        assert_eq!(
            b.rows[0][1],
            Value::Json(serde_json::json!({"verdict": "ok", "n": 3}))
        );
        let failed = SessionReport {
            outcome: Outcome::TurnsExhausted,
            ..report
        };
        let b = child_relation(&failed, false);
        assert_eq!(b.schema.names(), ["answer", "detail"]);
        assert_eq!(b.rows.len(), 1);
        assert_eq!(b.rows[0][0], Value::Null);
    }
}
