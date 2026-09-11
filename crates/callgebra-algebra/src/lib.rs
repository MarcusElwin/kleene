//! The call algebra.
//!
//! A [`CallPlan`] is a [`LogicalPlan`] whose operators are annotated with the
//! calls they imply ([`CallKind`]) and with cost estimates. The planner
//! applies rewrite rules that respect [`Volatility`] fences, and `EXPLAIN`
//! renders the result with estimated calls, tokens, dollars and depth per
//! node, plus the complexity fragment the query lives in.
//!
//! M2 implements annotation, the cost model, the cheap-first, dedupe,
//! semi-join and fence rules and `EXPLAIN`; M5 adds sampled selectivity,
//! cascades, beam-limited recursion and join ordering.

#![forbid(unsafe_code)]

mod annotate;
mod explain;
mod rules;

use callgebra_core::{CallKind, Catalog, Volatility};
use callgebra_sql::LogicalPlan;
use serde::{Deserialize, Serialize};

pub use annotate::{annotate, CostModel};
pub use explain::explain;
pub use rules::{CheapFirst, Fences, SemiJoin};

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

impl Fragment {
    /// Short badge text.
    pub fn badge(self) -> &'static str {
        match self {
            Fragment::Conjunctive => "CQ",
            Fragment::FirstOrder => "FO",
            Fragment::Recursive => "REC",
        }
    }

    /// One-line complexity note.
    pub fn note(self) -> &'static str {
        match self {
            Fragment::Conjunctive => "conjunctive query: NP-complete combined, AC0 data",
            Fragment::FirstOrder => "first-order: PSPACE-complete combined, AC0 data",
            Fragment::Recursive => "recursive: Datalog and beyond, PTIME data, EXPTIME program",
        }
    }
}

/// One node of the call plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallNode {
    /// The relational operator (children replaced by [`CallNode::children`]).
    pub op: LogicalPlan,
    /// Calls this node makes per input row, in evaluation order.
    pub calls: Vec<CallKind>,
    /// Most restrictive volatility of anything at this node.
    pub volatility: Volatility,
    /// Cost estimate for this node alone.
    pub estimate: Estimate,
    /// Children, mirroring the logical plan.
    pub children: Vec<CallNode>,
}

impl CallNode {
    /// Total estimate for this subtree.
    pub fn total(&self) -> Estimate {
        let mut t = self.estimate;
        for c in &self.children {
            t = t.plus(c.total());
        }
        t
    }

    /// Most restrictive volatility in the subtree.
    pub fn subtree_volatility(&self) -> Volatility {
        self.children
            .iter()
            .map(CallNode::subtree_volatility)
            .fold(self.volatility, Volatility::max)
    }
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

impl CallPlan {
    /// The logical plan after rewriting, for execution.
    pub fn logical(&self) -> LogicalPlan {
        annotate::to_logical(&self.root)
    }
}

/// A rewrite rule over call plans.
pub trait Rule: Send + Sync {
    /// Rule name for `EXPLAIN` and the trace.
    fn name(&self) -> &'static str;
    /// Apply once; return `Some` if the plan changed.
    fn apply(&self, plan: &CallNode, catalog: &Catalog) -> Option<CallNode>;
}

/// The default rule set, in application order.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(SemiJoin), Box::new(CheapFirst), Box::new(Fences)]
}

/// Build a call plan from a logical plan: annotate, apply the default rules
/// to a fixpoint (bounded), re-estimate.
pub fn plan(logical: &LogicalPlan, catalog: &Catalog, cost: &CostModel) -> CallPlan {
    plan_with(logical, catalog, cost, &default_rules())
}

/// [`plan`] with an explicit rule set.
pub fn plan_with(
    logical: &LogicalPlan,
    catalog: &Catalog,
    cost: &CostModel,
    rules: &[Box<dyn Rule>],
) -> CallPlan {
    let mut root = annotate(logical, catalog, cost);
    let mut applied = vec![];
    for _round in 0..8 {
        let mut changed = false;
        for r in rules {
            if let Some(next) = r.apply(&root, catalog) {
                root = annotate(&annotate::to_logical(&next), catalog, cost);
                applied.push(r.name().to_string());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let fragment = annotate::fragment(&root);
    let total = root.total();
    CallPlan {
        root,
        fragment,
        total,
        rules_applied: applied,
    }
}
