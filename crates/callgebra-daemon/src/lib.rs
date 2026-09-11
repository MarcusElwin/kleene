//! Daemon protocol.
//!
//! The engine runs as a daemon; the TUI and `--headless` clients attach over
//! a Unix socket and exchange newline-delimited JSON. Events carry a
//! [`Cursor`] so a reconnecting client resumes where it left off. Server and
//! client implementations arrive in M4; this crate fixes the wire types.

#![forbid(unsafe_code)]

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
        /// Task text or task id.
        task: String,
        /// Workspace path.
        workspace: String,
    },
    /// Submit SQL to a session (interactive REPL).
    Submit {
        /// Target session.
        session: SessionId,
        /// CallSQL text.
        sql: String,
    },
    /// Cancel a statement.
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
    /// Reply to `ListRuns`.
    Runs {
        /// Live runs.
        runs: Vec<RunId>,
    },
    /// The request failed.
    Error {
        /// Why.
        message: String,
    },
}

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 1;
