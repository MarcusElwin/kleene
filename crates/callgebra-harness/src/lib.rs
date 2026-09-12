//! The harness: sessions, the SQL REPL loop, agent roles and (M6) the
//! continual loop.
//!
//! A [`Session`] owns a catalog view, a budget and a transcript. Each turn the
//! model emits CallSQL; the harness parses, plans, checks the budget,
//! executes, renders the result back, and repeats until `FINAL`. Child
//! sessions are the same struct at depth + 1 with a role and a budget slice.
//! [`Harness`] runs sessions to `FINAL` (`callgebra run`), spawning children
//! for `rlm(...)` and `spawn(...)`; [`Repl`] runs single statements against a
//! store, which is what `callgebra repl` uses.

#![forbid(unsafe_code)]

pub mod live;
pub mod prompt;
pub mod repl;
pub mod session;
pub mod sink;
pub mod testing;

pub use live::{ChildRunner, LiveSink, ModelSettings, SessionMeta};
pub use repl::{Rendered, Repl, ReplConfig};
pub use session::{
    Harness, HarnessConfig, HarnessError, Observer, RunReport, SessionReport, SessionStatus, Turn,
};
pub use sink::StoreSink;

use callgebra_core::{Budget, Catalog, ModelAlias, SessionId};
use serde::{Deserialize, Serialize};

/// A declared agent role (`CREATE AGENT`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRole {
    /// Role name; `self` is the caller's own role.
    pub name: String,
    /// Model alias for the role's calls.
    pub model: ModelAlias,
    /// Effort level, if fixed.
    pub effort: Option<String>,
    /// Relations and functions the role may use; empty means the parent's view.
    pub tools: Vec<String>,
    /// Budget slice given to each child of this role.
    pub budget: Budget,
    /// System prompt appended to the shared harness prefix.
    pub prompt: String,
    /// Turn cap for sessions of this role, if fixed.
    pub max_turns: Option<u32>,
}

impl AgentRole {
    /// The root role: the caller's own model, no restrictions.
    pub fn root() -> Self {
        Self {
            name: "root".into(),
            model: ModelAlias::root(),
            effort: None,
            tools: vec![],
            budget: Budget::unbounded(),
            prompt: String::new(),
            max_turns: None,
        }
    }

    /// The anonymous child role used by `rlm(...)`: the parent's role one
    /// level deeper, on the worker tier.
    pub fn child_of(parent: &AgentRole) -> Self {
        Self {
            name: "self".into(),
            model: ModelAlias::worker(),
            effort: parent.effort.clone().or_else(|| Some("low".into())),
            tools: parent.tools.clone(),
            budget: Budget::unbounded(),
            prompt: parent.prompt.clone(),
            max_turns: None,
        }
    }
}

/// One model session, root or child.
#[derive(Debug, Clone)]
pub struct Session {
    /// Id.
    pub id: SessionId,
    /// Parent, if a child.
    pub parent: Option<SessionId>,
    /// Depth; root is 0.
    pub depth: u32,
    /// Role running this session.
    pub role: AgentRole,
    /// What this session can see.
    pub catalog: Catalog,
    /// What it may spend.
    pub budget: Budget,
}

/// Outcome of a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    /// `FINAL` was reached; these rows are the answer.
    Final {
        /// The answer relation.
        answer: callgebra_core::Batch,
    },
    /// A budget ran out before `FINAL`.
    BudgetExhausted {
        /// Which dimension.
        detail: String,
    },
    /// The turn cap was reached before `FINAL`; the session can be resumed.
    TurnsExhausted,
    /// The model or a tool failed in a way the session could not recover from.
    Failed {
        /// Error text.
        error: String,
    },
    /// The harness stopped it.
    Cancelled,
}

impl Outcome {
    /// Short tag used in the trace (`final`, `budget_exhausted`, ...).
    pub fn tag(&self) -> &'static str {
        match self {
            Outcome::Final { .. } => "final",
            Outcome::BudgetExhausted { .. } => "budget_exhausted",
            Outcome::TurnsExhausted => "turns_exhausted",
            Outcome::Failed { .. } => "error",
            Outcome::Cancelled => "cancelled",
        }
    }
}

/// How results are rendered back to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderOptions {
    /// Rows shown before truncation.
    pub max_rows: usize,
    /// Characters per cell before truncation.
    pub max_cell_chars: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            max_rows: 20,
            max_cell_chars: 200,
        }
    }
}
