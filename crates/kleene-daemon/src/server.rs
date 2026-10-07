//! The daemon: an event log every trace event is appended to, and a Unix
//! socket serving the protocol to any number of clients.

use crate::{ClientRequest, Cursor, DaemonError, ServerMessage, StatementOutput, PROTOCOL_VERSION};
use kleene_core::{Budget, RunId, SessionId, StatementId};
use kleene_harness::{Harness, HarnessConfig, Observer, Outcome, Repl, ReplConfig, SessionMeta};
use kleene_llm::ProviderSettings;
use kleene_store::DuckDbStore;
use kleene_tools::WebSearchBackend;
use kleene_trace::{FanoutSink, TraceEvent, TraceSink, Traced, Tracer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

/// Append-only log of server messages with cursors, plus a live broadcast.
pub struct EventLog {
    generation: u64,
    events: RwLock<Vec<ServerMessage>>,
    live: broadcast::Sender<(Cursor, ServerMessage)>,
    /// Which session each statement belongs to, for `Cancel`.
    statements: Mutex<HashMap<StatementId, SessionId>>,
}

impl EventLog {
    /// A fresh log for a new daemon incarnation.
    pub fn new() -> Self {
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(1);
        let (live, _) = broadcast::channel(4096);
        Self {
            generation,
            events: RwLock::new(Vec::new()),
            live,
            statements: Mutex::new(HashMap::new()),
        }
    }

    /// The incarnation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Append a message and broadcast it. Returns its cursor.
    pub fn push(&self, msg: ServerMessage) -> Cursor {
        if let ServerMessage::Event { traced, .. } = &msg {
            if let TraceEvent::StatementStarted {
                session, statement, ..
            } = &traced.event
            {
                if let Ok(mut m) = self.statements.lock() {
                    m.insert(*statement, *session);
                }
            }
        }
        let cursor = {
            let mut events = self.events.write().unwrap_or_else(|e| e.into_inner());
            let cursor = Cursor {
                generation: self.generation,
                sequence: events.len() as u64,
            };
            let msg = match msg {
                ServerMessage::Event { traced, .. } => ServerMessage::Event { cursor, traced },
                other => other,
            };
            events.push(msg.clone());
            let _ = self.live.send((cursor, msg));
            cursor
        };
        cursor
    }

    /// Record a trace event.
    pub fn record(&self, traced: Traced) -> Cursor {
        self.push(ServerMessage::Event {
            cursor: Cursor::default(),
            traced,
        })
    }

    /// Everything after `after` (or everything, for `None`), with cursors.
    pub fn replay(&self, after: Option<Cursor>) -> Vec<(Cursor, ServerMessage)> {
        let events = self.events.read().unwrap_or_else(|e| e.into_inner());
        let start = match after {
            Some(c) if c.generation == self.generation => (c.sequence + 1) as usize,
            _ => 0,
        };
        events
            .iter()
            .enumerate()
            .skip(start)
            .map(|(i, m)| {
                (
                    Cursor {
                        generation: self.generation,
                        sequence: i as u64,
                    },
                    m.clone(),
                )
            })
            .collect()
    }

    /// Number of messages logged.
    pub fn len(&self) -> usize {
        self.events.read().map(|e| e.len()).unwrap_or(0)
    }

    /// Whether nothing has been logged.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Subscribe to live messages.
    pub fn subscribe(&self) -> broadcast::Receiver<(Cursor, ServerMessage)> {
        self.live.subscribe()
    }

    /// The session a statement ran in, if seen.
    pub fn session_of(&self, statement: StatementId) -> Option<SessionId> {
        self.statements
            .lock()
            .ok()
            .and_then(|m| m.get(&statement).copied())
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

/// A trace sink that appends to an [`EventLog`].
pub struct LogSink(pub Arc<EventLog>);

impl TraceSink for LogSink {
    fn record(&self, event: Traced) {
        self.0.record(event);
    }
}

struct LiveRun {
    handle: tokio::task::JoinHandle<()>,
}

/// The engine daemon.
pub struct Daemon {
    log: Arc<EventLog>,
    /// The harness and the config it was built from; `Reload` replaces both.
    live: RwLock<(Arc<Harness>, HarnessConfig)>,
    store: DuckDbStore,
    trace: kleene_store::DuckDbTraceSink,
    runs: Mutex<HashMap<RunId, LiveRun>>,
    repls: tokio::sync::Mutex<HashMap<SessionId, Arc<Repl>>>,
}

/// Forwards streamed model text into the event log as
/// [`ServerMessage::CallDelta`], so the TUI's live pane fills as the model
/// writes.
pub struct LogObserver(pub Arc<EventLog>);

impl Observer for LogObserver {
    fn call_delta(&self, _meta: &SessionMeta, call: kleene_core::CallId, text: &str) {
        self.0.push(ServerMessage::CallDelta {
            call,
            text: text.to_string(),
        });
    }

    fn turn(&self, meta: &SessionMeta, turn: &kleene_harness::Turn) {
        self.0.push(ServerMessage::TurnFinished {
            session: meta.id,
            turn: turn.n,
            reply: turn.reply.clone(),
            sql: turn.sql.clone(),
            results: turn
                .results
                .iter()
                .map(|r| StatementOutput {
                    text: r.text.clone(),
                    is_error: r.is_error,
                    is_final: r.is_final,
                })
                .collect(),
            plan: turn.plan.clone(),
        });
    }
}

impl Daemon {
    /// Build a daemon over a store. `cfg.tracer` is replaced by a fan-out to
    /// the store's trace sink and the daemon's event log.
    pub async fn new(store: DuckDbStore, mut cfg: HarnessConfig) -> Result<Arc<Self>, DaemonError> {
        let log = Arc::new(EventLog::new());
        let trace = store.trace_sink();
        let sinks: Vec<Arc<dyn TraceSink>> =
            vec![Arc::new(trace.clone()), Arc::new(LogSink(log.clone()))];
        cfg.tracer = Some(Tracer::new(Arc::new(FanoutSink::new(sinks))));
        if cfg.observer.is_none() {
            cfg.observer = Some(Arc::new(LogObserver(log.clone())));
        }
        let harness = Harness::new(store.clone(), cfg.clone()).await?;
        Ok(Arc::new(Self {
            log,
            live: RwLock::new((harness, cfg)),
            store,
            trace,
            runs: Mutex::new(HashMap::new()),
            repls: tokio::sync::Mutex::new(HashMap::new()),
        }))
    }

    /// The event log.
    pub fn log(&self) -> &Arc<EventLog> {
        &self.log
    }

    /// The harness runs and sessions are started on now.
    pub fn harness(&self) -> Arc<Harness> {
        self.live
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .clone()
    }

    /// The config runs and sessions are started with now.
    pub fn config(&self) -> HarnessConfig {
        self.live
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }

    /// Rebuild the provider and the web search backend from the config file
    /// and the environment ([`ProviderSettings::effective`]); everything
    /// else in the config is kept. Returns what is configured, for the
    /// `Ok` reply.
    async fn reload(&self) -> Result<String, String> {
        let settings = ProviderSettings::effective().map_err(|e| e.to_string())?;
        let provider = kleene_llm::provider_from_settings(&settings).map_err(|e| e.to_string())?;
        let web_search = settings.web_search_configured().and_then(|w| {
            WebSearchBackend::new(w.provider_name(), w.api_key.as_deref().unwrap_or(""))
        });
        let mut cfg = self.config();
        cfg.provider = Some(provider);
        cfg.web_search = web_search;
        let config_dir = cfg.config_dir.clone().unwrap_or_else(ProviderSettings::dir);
        cfg.mcp = kleene_tools::mcp::load_all(&cfg.workspace, &config_dir).0;
        cfg.skills = None;
        let harness = Harness::new(self.store.clone(), cfg.clone())
            .await
            .map_err(|e| e.to_string())?;
        let mut summary: Vec<String> = settings.configured().map(str::to_string).collect();
        if let Some(w) = &cfg.web_search {
            summary.push(format!("web search {}", w.provider));
        }
        for m in harness.mcp_status() {
            summary.push(format!(
                "mcp {} {}",
                m.server,
                if m.state == "connected" {
                    format!("({} tools)", m.tools.len())
                } else {
                    m.state.clone()
                }
            ));
        }
        summary.push(format!("{} skills", harness.skills().len()));
        *self.live.write().unwrap_or_else(|e| e.into_inner()) = (harness, cfg);
        Ok(format!("reloaded: {}", summary.join(", ")))
    }

    /// Bind the socket (replacing a stale file) and serve forever.
    pub async fn serve(self: Arc<Self>, socket: &Path) -> Result<(), DaemonError> {
        let listener = bind(socket)?;
        loop {
            let (stream, _) = listener.accept().await?;
            let me = self.clone();
            tokio::spawn(async move {
                if let Err(e) = me.handle(stream).await {
                    if !matches!(e, DaemonError::Closed) {
                        tracing::warn!("client connection ended: {e}");
                    }
                }
            });
        }
    }

    /// Serve until `shutdown` resolves.
    pub async fn serve_until(
        self: Arc<Self>,
        socket: &Path,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), DaemonError> {
        let listener = bind(socket)?;
        let mut shutdown = Box::pin(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let me = self.clone();
                    tokio::spawn(async move {
                        let _ = me.handle(stream).await;
                    });
                }
            }
        }
    }

    /// Handle one client until it detaches or disconnects.
    pub async fn handle(self: Arc<Self>, stream: UnixStream) -> Result<(), DaemonError> {
        let (read, mut write) = stream.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let mut line = serde_json::to_string(&msg).unwrap_or_default();
                line.push('\n');
                if write.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        let _ = tx.send(ServerMessage::Hello {
            protocol: PROTOCOL_VERSION,
            generation: self.log.generation(),
        });
        let mut lines = BufReader::new(read).lines();
        let mut forwarder: Option<tokio::task::JoinHandle<()>> = None;
        let result = loop {
            let line = match lines.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => break Err(DaemonError::Closed),
                Err(e) => break Err(DaemonError::Io(e)),
            };
            if line.trim().is_empty() {
                continue;
            }
            let req: ClientRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(ServerMessage::Error {
                        message: format!("bad request: {e}"),
                    });
                    continue;
                }
            };
            match req {
                ClientRequest::Detach => break Ok(()),
                ClientRequest::Subscribe { after } => {
                    if let Some(f) = forwarder.take() {
                        f.abort();
                    }
                    // Replay, then forward live messages; anything that
                    // arrives between the two is deduplicated by cursor.
                    let mut live = self.log.subscribe();
                    let replayed = self.log.replay(after);
                    let mut last = replayed.last().map(|(c, _)| *c);
                    for (_, m) in replayed {
                        let _ = tx.send(m);
                    }
                    let tx2 = tx.clone();
                    forwarder = Some(tokio::spawn(async move {
                        loop {
                            match live.recv().await {
                                Ok((cursor, msg)) => {
                                    if last.is_some_and(|l| cursor <= l) {
                                        continue;
                                    }
                                    last = Some(cursor);
                                    if tx2.send(msg).is_err() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    }));
                }
                other => {
                    let reply = self.clone().dispatch(other).await;
                    // A subscribed client already receives `RunAccepted`
                    // through the log; do not send it twice.
                    let duplicate =
                        forwarder.is_some() && matches!(reply, ServerMessage::RunAccepted { .. });
                    if !duplicate {
                        let _ = tx.send(reply);
                    }
                }
            }
        };
        if let Some(f) = forwarder {
            f.abort();
        }
        drop(tx);
        let _ = writer.await;
        result
    }

    /// Handle one request (everything but `Subscribe` and `Detach`).
    pub async fn dispatch(self: Arc<Self>, req: ClientRequest) -> ServerMessage {
        match req {
            ClientRequest::StartRun {
                task,
                workspace,
                context,
                max_turns,
                max_depth,
                budget_calls,
                check,
            } => {
                let (live_harness, live_cfg) = {
                    let live = self.live.read().unwrap_or_else(|e| e.into_inner());
                    (live.0.clone(), live.1.clone())
                };
                let mut cfg = live_cfg.clone();
                if !workspace.is_empty() {
                    cfg.workspace = PathBuf::from(workspace);
                }
                if let Some(t) = max_turns {
                    cfg.max_turns = t;
                }
                if let Some(d) = max_depth {
                    cfg.max_depth = d;
                }
                if let Some(c) = budget_calls {
                    cfg.budget = Budget {
                        calls: Some(c),
                        ..cfg.budget
                    };
                }
                if check.is_some() {
                    cfg.finish_check = check;
                }
                let harness = if cfg.workspace == live_cfg.workspace
                    && cfg.max_turns == live_cfg.max_turns
                    && cfg.max_depth == live_cfg.max_depth
                    && cfg.budget == live_cfg.budget
                    && cfg.finish_check == live_cfg.finish_check
                {
                    live_harness
                } else {
                    match Harness::new(self.store.clone(), cfg).await {
                        Ok(h) => h,
                        Err(e) => {
                            return ServerMessage::Error {
                                message: e.to_string(),
                            }
                        }
                    }
                };
                let run = RunId::new();
                let me = self.clone();
                let handle = tokio::spawn(async move {
                    let outcome = harness.run_as(run, &task, context).await;
                    let (tag, answer, answer_columns, answer_rows) = match &outcome {
                        Ok(report) => match &report.root.outcome {
                            Outcome::Final { answer } => {
                                let (columns, rows) = answer_cells(answer, 200);
                                (
                                    report.root.outcome.tag().to_string(),
                                    Some(answer.render_table(50)),
                                    columns,
                                    rows,
                                )
                            }
                            other => (other.tag().to_string(), None, vec![], vec![]),
                        },
                        Err(e) => (format!("error: {e}"), None, vec![], vec![]),
                    };
                    me.log.push(ServerMessage::RunFinished {
                        run,
                        outcome: tag,
                        answer,
                        answer_columns,
                        answer_rows,
                    });
                    if let Ok(mut runs) = me.runs.lock() {
                        runs.remove(&run);
                    }
                });
                if let Ok(mut runs) = self.runs.lock() {
                    runs.insert(run, LiveRun { handle });
                }
                self.log.push(ServerMessage::RunAccepted { run });
                ServerMessage::RunAccepted { run }
            }
            ClientRequest::Submit { session, sql } => {
                let repl = {
                    let mut repls = self.repls.lock().await;
                    match repls.get(&session) {
                        Some(r) => r.clone(),
                        None => {
                            let live = self.config();
                            let cfg = ReplConfig {
                                workspace: live.workspace,
                                provider: live.provider,
                                tracer: live.tracer,
                                web_search: live.web_search,
                            };
                            match Repl::with_config(self.store.clone(), cfg).await {
                                Ok(mut r) => {
                                    r.set_session(session);
                                    let r = Arc::new(r);
                                    repls.insert(session, r.clone());
                                    r
                                }
                                Err(e) => {
                                    return ServerMessage::Error {
                                        message: e.to_string(),
                                    }
                                }
                            }
                        }
                    }
                };
                let results = repl
                    .submit(&sql)
                    .await
                    .into_iter()
                    .map(|r| StatementOutput {
                        text: r.text,
                        is_error: r.is_error,
                        is_final: r.is_final,
                    })
                    .collect();
                ServerMessage::Submitted { session, results }
            }
            ClientRequest::Query { sql, tag } => {
                // The trace writer is asynchronous; make it catch up so the
                // explorer sees everything the event stream already showed.
                self.trace.flush().await;
                match self.store.query(&sql).await {
                    Ok(batch) => ServerMessage::Table {
                        columns: batch.schema.names().iter().map(|s| s.to_string()).collect(),
                        rows: batch
                            .rows
                            .iter()
                            .map(|r| r.iter().map(|v| v.render()).collect())
                            .collect(),
                        tag,
                    },
                    Err(e) => ServerMessage::Error {
                        message: e.to_string(),
                    },
                }
            }
            ClientRequest::Cancel { statement } => {
                let Some(session) = self.log.session_of(statement) else {
                    return ServerMessage::Error {
                        message: format!("unknown statement {statement}"),
                    };
                };
                let cancelled = self.harness().cancel_statement(session)
                    || match self.repls.lock().await.get(&session) {
                        Some(r) => {
                            r.sink().cancel_statement();
                            true
                        }
                        None => false,
                    };
                if cancelled {
                    ServerMessage::Ok {
                        message: format!("cancelling statement {statement}"),
                    }
                } else {
                    ServerMessage::Error {
                        message: format!("session {session} is not running"),
                    }
                }
            }
            ClientRequest::CancelRun { run } => {
                let live = self.runs.lock().ok().and_then(|mut r| r.remove(&run));
                match live {
                    Some(l) => {
                        l.handle.abort();
                        self.log.push(ServerMessage::RunFinished {
                            run,
                            outcome: "cancelled".into(),
                            answer: None,
                            answer_columns: vec![],
                            answer_rows: vec![],
                        });
                        ServerMessage::Ok {
                            message: format!("cancelled run {run}"),
                        }
                    }
                    None => ServerMessage::Error {
                        message: format!("no live run {run}"),
                    },
                }
            }
            ClientRequest::ListMcp => {
                let harness = self.harness();
                let rows = harness
                    .mcp_status()
                    .iter()
                    .map(|m| {
                        vec![
                            m.server.clone(),
                            m.state.clone(),
                            m.tools.join(", "),
                            m.command.clone(),
                        ]
                    })
                    .collect();
                ServerMessage::Table {
                    columns: ["server", "state", "tools", "command"]
                        .iter()
                        .map(|c| c.to_string())
                        .collect(),
                    rows,
                    tag: Some("mcp".into()),
                }
            }
            ClientRequest::ListSkills { name } => {
                let harness = self.harness();
                match name {
                    None => ServerMessage::Table {
                        columns: ["name", "source", "description"]
                            .iter()
                            .map(|c| c.to_string())
                            .collect(),
                        rows: harness
                            .skills()
                            .iter()
                            .map(|s| vec![s.name.clone(), s.source.clone(), s.description.clone()])
                            .collect(),
                        tag: Some("skills".into()),
                    },
                    Some(name) => match harness
                        .skills()
                        .iter()
                        .find(|s| s.name.eq_ignore_ascii_case(name.trim()))
                    {
                        Some(s) => ServerMessage::Table {
                            columns: vec!["text".into()],
                            rows: vec![vec![format!(
                                "{} ({}, {})\n\n{}",
                                s.name, s.source, s.path, s.body
                            )]],
                            tag: Some("skill".into()),
                        },
                        None => ServerMessage::Error {
                            message: format!("no skill named {name:?}; /skills lists them"),
                        },
                    },
                }
            }
            ClientRequest::Reload => match self.reload().await {
                Ok(message) => ServerMessage::Ok { message },
                Err(message) => ServerMessage::Error { message },
            },
            ClientRequest::ListRuns => ServerMessage::Runs {
                runs: self
                    .runs
                    .lock()
                    .map(|r| r.keys().copied().collect())
                    .unwrap_or_default(),
            },
            ClientRequest::Subscribe { .. } | ClientRequest::Detach => ServerMessage::Ok {
                message: "handled by the connection".into(),
            },
        }
    }
}

/// The answer relation's column names and its first `max_rows` rows with
/// every cell rendered in full, for `RunFinished`.
fn answer_cells(answer: &kleene_core::Batch, max_rows: usize) -> (Vec<String>, Vec<Vec<String>>) {
    let columns = answer
        .schema
        .names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    let rows = answer
        .rows
        .iter()
        .take(max_rows)
        .map(|r| r.iter().map(kleene_core::Value::render).collect())
        .collect();
    (columns, rows)
}

fn bind(socket: &Path) -> Result<UnixListener, DaemonError> {
    if socket.exists() {
        // A live daemon would answer; a dead one leaves the file behind.
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Err(DaemonError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("a daemon is already listening on {}", socket.display()),
            )));
        }
        std::fs::remove_file(socket)?;
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(UnixListener::bind(socket)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_respects_cursors_and_generations() {
        let log = EventLog::new();
        for i in 0..3 {
            log.push(ServerMessage::Ok {
                message: format!("m{i}"),
            });
        }
        assert_eq!(log.replay(None).len(), 3);
        let after = Cursor {
            generation: log.generation(),
            sequence: 0,
        };
        let rest = log.replay(Some(after));
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].0.sequence, 1);
        let stale = Cursor {
            generation: log.generation() - 1,
            sequence: 2,
        };
        assert_eq!(
            log.replay(Some(stale)).len(),
            3,
            "stale generation replays all"
        );
    }
}
