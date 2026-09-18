//! Trace events.
//!
//! Every event is append-only and self-describing, so a sink can be a file,
//! stderr, the DuckDB store (M1) or the daemon's event stream (M4). The event
//! set is the schema of the `trace_*` relations the model can query.

#![forbid(unsafe_code)]

use kleene_core::{BudgetUsage, CallId, RunId, SessionId, StatementId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// One trace event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum TraceEvent {
    /// A run began.
    RunStarted {
        /// Run.
        run: RunId,
        /// Task description or id.
        task: String,
    },
    /// A session began (root or child).
    SessionStarted {
        /// Run.
        run: RunId,
        /// Session.
        session: SessionId,
        /// Parent session, if any.
        parent: Option<SessionId>,
        /// Depth (root is 0).
        depth: u32,
        /// Agent role name.
        role: String,
        /// The task the session was given.
        task: String,
    },
    /// A session ended (with `FINAL`, an exhausted budget, a turn cap or an
    /// error).
    SessionFinished {
        /// Session.
        session: SessionId,
        /// `final`, `budget_exhausted`, `turns_exhausted`, `cancelled` or `error`.
        outcome: String,
        /// Model turns taken.
        turns: u32,
        /// Usage of the session including its children.
        usage: BudgetUsage,
    },
    /// A statement was submitted.
    StatementStarted {
        /// Session.
        session: SessionId,
        /// Statement.
        statement: StatementId,
        /// SQL text.
        sql: String,
    },
    /// A statement was planned; `explain` is the rendered plan and
    /// `estimate` the JSON of the total estimate.
    StatementPlanned {
        /// Statement.
        statement: StatementId,
        /// Rendered `EXPLAIN`.
        explain: String,
        /// Serialised estimate.
        estimate: serde_json::Value,
    },
    /// A statement finished, with or without error.
    StatementFinished {
        /// Statement.
        statement: StatementId,
        /// Rows produced.
        rows: u64,
        /// Usage attributed to this statement (children included).
        usage: BudgetUsage,
        /// Error text, if it failed.
        error: Option<String>,
        /// Wall time.
        elapsed: Duration,
    },
    /// A model call started.
    CallStarted {
        /// Statement issuing it.
        statement: StatementId,
        /// Call.
        call: CallId,
        /// Model alias.
        alias: String,
        /// Concrete model, once routed.
        model: String,
        /// Request fingerprint (memo key).
        fingerprint: String,
    },
    /// A model call finished.
    CallFinished {
        /// Call.
        call: CallId,
        /// Input tokens.
        input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
        /// Cache read tokens.
        cache_read_tokens: u64,
        /// Dollars.
        cost_usd: f64,
        /// Wall time.
        elapsed: Duration,
        /// Served from the memo rather than the provider.
        memo_hit: bool,
        /// Error text, if it failed.
        error: Option<String>,
    },
    /// A tool ran.
    ToolCall {
        /// Statement issuing it.
        statement: StatementId,
        /// Tool name.
        tool: String,
        /// Arguments rendered as text.
        args: String,
        /// Output bytes.
        bytes_out: u64,
        /// Wall time.
        elapsed: Duration,
        /// Error text, if it failed.
        error: Option<String>,
    },
    /// One semi-naive round of a recursive query.
    RecursionRound {
        /// Statement.
        statement: StatementId,
        /// CTE name.
        cte: String,
        /// Round number (0 is the base term).
        round: u32,
        /// New rows this round.
        delta_rows: u64,
        /// Accumulated rows.
        total_rows: u64,
    },
    /// A budget dimension was exceeded and the statement cancelled.
    BudgetExceeded {
        /// Statement.
        statement: StatementId,
        /// Which dimension and by how much.
        detail: String,
    },
    /// A session ended with `FINAL`.
    Final {
        /// Session.
        session: SessionId,
        /// Rows in the answer.
        rows: u64,
    },
}

/// A timestamped event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Traced {
    /// When it happened.
    pub at: SystemTime,
    /// The event.
    pub event: TraceEvent,
}

/// Where events go.
pub trait TraceSink: Send + Sync {
    /// Record one event. Must not block for long; sinks buffer.
    fn record(&self, event: Traced);
}

/// Writes events as JSON lines to stderr via `tracing`. The default sink.
pub struct LogSink;

impl TraceSink for LogSink {
    fn record(&self, event: Traced) {
        match serde_json::to_string(&event) {
            Ok(json) => tracing::info!(target: "kleene::trace", "{json}"),
            Err(e) => tracing::warn!(target: "kleene::trace", "unserialisable event: {e}"),
        }
    }
}

/// Collects events in memory, for tests.
#[derive(Default)]
pub struct VecSink {
    events: std::sync::Mutex<Vec<Traced>>,
}

impl VecSink {
    /// Empty sink.
    pub fn new() -> Self {
        Self::default()
    }
    /// Everything recorded so far.
    pub fn events(&self) -> Vec<Traced> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }
}

impl TraceSink for VecSink {
    fn record(&self, event: Traced) {
        if let Ok(mut e) = self.events.lock() {
            e.push(event);
        }
    }
}

/// Sends every event to several sinks (the store and a live subscriber, say).
pub struct FanoutSink {
    sinks: Vec<Arc<dyn TraceSink>>,
}

impl FanoutSink {
    /// Fan out to `sinks`, in order.
    pub fn new(sinks: Vec<Arc<dyn TraceSink>>) -> Self {
        Self { sinks }
    }
}

impl TraceSink for FanoutSink {
    fn record(&self, event: Traced) {
        for s in &self.sinks {
            s.record(event.clone());
        }
    }
}

/// Convenience handle shared by everything that emits events.
#[derive(Clone)]
pub struct Tracer {
    sink: Arc<dyn TraceSink>,
}

impl Tracer {
    /// Wrap a sink.
    pub fn new(sink: Arc<dyn TraceSink>) -> Self {
        Self { sink }
    }
    /// Record an event now.
    pub fn emit(&self, event: TraceEvent) {
        self.sink.record(Traced {
            at: SystemTime::now(),
            event,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_and_reach_the_sink() {
        let sink = Arc::new(VecSink::new());
        let t = Tracer::new(sink.clone());
        let run = RunId::new();
        t.emit(TraceEvent::RunStarted {
            run,
            task: "demo".into(),
        });
        let evs = sink.events();
        assert_eq!(evs.len(), 1);
        let json = serde_json::to_string(&evs[0]).unwrap();
        let back: Traced = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event, evs[0].event);
    }
}
