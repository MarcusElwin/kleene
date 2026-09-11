//! The harness: sessions, the SQL REPL loop, agent roles and (M6) the
//! continual loop.
//!
//! A [`Session`] owns a catalog view, a budget and a transcript. Each turn the
//! model emits CallSQL; the harness parses, plans, checks the budget,
//! executes, renders the result back, and repeats until `FINAL`. Child
//! sessions are the same struct at depth + 1 with a role and a budget slice.
//! Implemented in M3.

#![forbid(unsafe_code)]

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
    /// `FINAL` was reached; the answer relation is in the store under this name.
    Final {
        /// Table holding the answer.
        table: String,
    },
    /// A budget ran out before `FINAL`.
    BudgetExhausted {
        /// Which dimension.
        detail: String,
    },
    /// The harness stopped it.
    Cancelled,
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
