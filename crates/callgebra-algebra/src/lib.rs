//! The call algebra.
//!
//! A [`CallPlan`] is a [`LogicalPlan`] whose operators are annotated with the
//! calls they imply ([`CallKind`]) and with cost estimates. The planner applies
//! rewrite rules that respect [`Volatility`] fences, and `EXPLAIN` renders the
//! result with estimated calls, tokens, dollars and depth per node.
//!
//! Implemented in M2 (rules 1-3, 5, 8) and M5 (cost model, cascades, join
//! ordering). This crate defines the types and the rule interface.

#![forbid(unsafe_code)]

use callgebra_core::{CallKind, Catalog, Volatility};
use callgebra_sql::LogicalPlan;
use serde::{Deserialize, Serialize};

/// Estimated or measured cost of one operator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Estimate {
    /// Output rows.
    pub rows: f64,
    /// Model calls.
    pub calls: f64,
    /// Tokens (input plus output).
    pub tokens: f64,
    /// Dollars.
    pub dollars: f64,
    /// Recursion depth reached below this node.
    pub depth: u32,
}

impl Estimate {
    /// Sum two estimates (rows are not additive; the caller sets them).
    pub fn plus(self, other: Estimate) -> Estimate {
        Estimate {
            rows: self.rows,
            calls: self.calls + other.calls,
            tokens: self.tokens + other.tokens,
            dollars: self.dollars + other.dollars,
            depth: self.depth.max(other.depth),
        }
    }
}

/// Which complexity fragment a query lives in. Shown as a badge in `EXPLAIN`
/// and the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fragment {
    /// Select-project-join with no negation: conjunctive query.
    Conjunctive,
    /// Full relational algebra (negation, aggregation, no recursion).
    FirstOrder,
    /// Contains a recursive CTE.
    Recursive,
}

/// One node of the call plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallNode {
    /// The relational operator.
    pub op: LogicalPlan,
    /// Calls this node makes, in evaluation order.
    pub calls: Vec<CallKind>,
    /// Most restrictive volatility of anything under this node.
    pub volatility: Volatility,
    /// Cost estimate for this node alone.
    pub estimate: Estimate,
    /// Children, mirroring the logical plan.
    pub children: Vec<CallNode>,
}

/// A planned statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallPlan {
    /// Root node.
    pub root: CallNode,
    /// Fragment class of the whole query.
    pub fragment: Fragment,
    /// Total estimate.
    pub total: Estimate,
    /// Rewrite rules that fired, in order.
    pub rules_applied: Vec<String>,
}

/// A rewrite rule over call plans.
pub trait Rule: Send + Sync {
    /// Rule name for `EXPLAIN` and the trace.
    fn name(&self) -> &'static str;
    /// Apply once; return `Some` if the plan changed.
    fn apply(&self, plan: &CallNode, catalog: &Catalog) -> Option<CallNode>;
}

/// Build a call plan from a logical plan.
///
/// Implemented in M2.
pub fn plan(_logical: &LogicalPlan, _catalog: &Catalog) -> CallPlan {
    todo!("M2: annotate operators with call kinds and estimates")
}

/// Render a call plan as an indented tree with per-node estimates.
///
/// Implemented in M2.
pub fn explain(_plan: &CallPlan) -> String {
    todo!("M2: EXPLAIN rendering")
}
