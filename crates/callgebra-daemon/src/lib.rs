//! Daemon protocol, server and client.
//!
//! The engine runs as a daemon; the TUI and `--headless` clients attach over
//! a Unix socket and exchange newline-delimited JSON. Every trace event the
//! engine emits is appended to an in-memory log with a [`Cursor`], so a
//! reconnecting client resumes where it left off, and mirrored into the
//! DuckDB store as before. [`server::Daemon`] serves the protocol;
//! [`client::Client`] speaks it.

#![forbid(unsafe_code)]

pub mod client;
pub mod server;

use callgebra_core::{RunId, SessionId, StatementId};
use callgebra_trace::Traced;
use serde::{Deserialize, Serialize};

/// Position in the event stream. `generation` changes when the daemon
/// restarts, so a stale cursor is detected rather than misapplied.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Cursor {
    /// Daemon incarnation.
    pub generation: u64,
    /// Event index within the generation.
    pub sequence: u64,
}

/// Client to daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "request")]
pub enum ClientRequest {
    /// Subscribe to events after a cursor (or from the start).
    Subscribe {
        /// Resume point.
        after: Option<Cursor>,
    },
    /// Start a run.
    StartRun {
        /// Task text.
        task: String,
        /// Workspace path (empty for the daemon's default).
        workspace: String,
        /// Context text loaded as `ctx`, if any.
        #[serde(default)]
        context: Option<String>,
        /// Turn cap for the root session.
        #[serde(default)]
        max_turns: Option<u32>,
        /// Deepest child session.
        #[serde(default)]
        max_depth: Option<u32>,
        /// Call budget.
        #[serde(default)]
        budget_calls: Option<u64>,
    },
    /// Submit SQL to a session (interactive REPL). An unknown session id
    /// opens a fresh interactive session under that id.
    Submit {
        /// Target session.
        session: SessionId,
        /// CallSQL text.
        sql: String,
    },
    /// Run DuckDB SQL over the store (trace explorer).
    Query {
        /// SQL.
        sql: String,
    },
    /// Cancel a statement (the session continues; the model sees the error).
    Cancel {
        /// Statement.
        statement: StatementId,
    },
    /// Cancel a whole run.
    CancelRun {
        /// Run.
        run: RunId,
    },
    /// List live runs.
    ListRuns,
    /// Detach without stopping anything.
    Detach,
}

/// One statement's outcome, as the REPL rendered it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementOutput {
    /// Rendered text.
    pub text: String,
    /// It failed.
    pub is_error: bool,
    /// It was `FINAL`.
    pub is_final: bool,
}

/// Daemon to client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "msg")]
pub enum ServerMessage {
    /// Handshake.
    Hello {
        /// Protocol version.
        protocol: u32,
        /// Current generation.
        generation: u64,
    },
    /// A trace event with its cursor.
    Event {
        /// Position.
        cursor: Cursor,
        /// The event.
        traced: Traced,
    },
    /// Streamed text from a model call, for the live pane.
    CallDelta {
        /// Which call.
        call: callgebra_core::CallId,
        /// Text chunk.
        text: String,
    },
    /// A run was accepted.
    RunAccepted {
        /// Its id.
        run: RunId,
    },
    /// A run ended.
    RunFinished {
        /// Run.
        run: RunId,
        /// `final`, `budget_exhausted`, `turns_exhausted`, `error`, `cancelled`.
        outcome: String,
        /// The answer relation rendered as text, when there is one.
        answer: Option<String>,
    },
    /// Reply to `Submit`.
    Submitted {
        /// Session.
        session: SessionId,
        /// One entry per statement that ran.
        results: Vec<StatementOutput>,
    },
    /// Reply to `Query`.
    Table {
        /// Column names.
        columns: Vec<String>,
        /// Rows rendered as text.
        rows: Vec<Vec<String>>,
    },
    /// Reply to `ListRuns`.
    Runs {
        /// Live runs.
        runs: Vec<RunId>,
    },
    /// A request succeeded with nothing else to say.
    Ok {
        /// What happened.
        message: String,
    },
    /// The request failed.
    Error {
        /// Why.
        message: String,
    },
}

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 2;

/// Daemon errors.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Socket or pipe failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A line was not valid protocol JSON.
    #[error("protocol: {0}")]
    Protocol(String),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] callgebra_store::StoreError),
    /// The harness failed.
    #[error(transparent)]
    Harness(#[from] callgebra_harness::HarnessError),
    /// The peer went away.
    #[error("connection closed")]
    Closed,
}
