//! The call algebra.
//!
//! A [`CallPlan`] is a [`LogicalPlan`] whose operators are annotated with the
//! calls they imply ([`CallKind`]) and with cost estimates. The planner
//! applies rewrite rules that respect [`Volatility`] fences, and `EXPLAIN`
//! renders the result with estimated calls, tokens, dollars and depth per
//! node, plus the complexity fragment the query lives in.
//!
//! Rules: join ordering over call predicates (dynamic programming over
//! relation subsets with branch-and-bound, run once), cascades through
//! declared proxies, semi-joins for `EXISTS`, cheap-first conjunct ordering
//! and volatility fences. Estimates use sampled selectivity (observed pass
//! rates of call predicates), per-alias cost factors and beam-aware
//! recursion costing.

#![forbid(unsafe_code)]

mod annotate;
mod explain;
mod rules;

use kleene_core::{CallKind, Catalog, Volatility};
use kleene_sql::LogicalPlan;
use serde::{Deserialize, Serialize};

pub use annotate::{annotate, beam_of, plan_call_names, rounds_of, CostModel};
pub use explain::explain;
pub use rules::{Cascade, CheapFirst, Fences, JoinOrder, SemiJoin};

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

/// A plan the planner considered and priced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alternative {
    /// What it is (`as written`, `chosen`, a join order, ...).
    pub label: String,
    /// Its estimated total.
    pub estimate: Estimate,
    /// Whether it is the plan that was kept.
    pub chosen: bool,
}

/// How big the search was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlanSpace {
    /// Relations joined.
    pub relations: usize,
    /// Distinct bushy join orders in the space.
    pub orders: u64,
    /// Subset splits actually priced.
    pub evaluated: u64,
    /// Splits skipped because their inputs already cost more than the best plan.
    pub pruned: u64,
}

/// What a rule produced: the new tree plus what it learned on the way.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    /// The rewritten node.
    pub node: CallNode,
    /// Plans considered, if the rule searched.
    pub alternatives: Vec<Alternative>,
    /// Search size, if the rule searched.
    pub plan_space: Option<PlanSpace>,
}

impl Rewrite {
    /// A plain rewrite with nothing to report.
    pub fn node(node: CallNode) -> Self {
        Self {
            node,
            alternatives: vec![],
            plan_space: None,
        }
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
    /// Plans considered and priced (the chosen one included).
    #[serde(default)]
    pub alternatives: Vec<Alternative>,
    /// Size of the join-order search, when one ran.
    #[serde(default)]
    pub plan_space: Option<PlanSpace>,
    /// The learned estimates the plan relies on, one line each
    /// ([`CostModel::learned_notes`]).
    #[serde(default)]
    pub learned: Vec<String>,
}

impl CallPlan {
    /// The logical plan after rewriting, for execution.
    pub fn logical(&self) -> LogicalPlan {
        annotate::to_logical(&self.root)
    }
}

/// A rewrite rule over call plans.
pub trait Rule: Send + Sync {
    /// Name, for `rules_applied`.
    fn name(&self) -> &'static str;
    /// Whether the rule runs once, before the fixpoint loop (searches that
    /// would otherwise re-run on their own output).
    fn once(&self) -> bool {
        false
    }
    /// Try to rewrite; `None` when nothing applies.
    fn apply(&self, plan: &CallNode, catalog: &Catalog, cost: &CostModel) -> Option<Rewrite>;
}

/// The default rule set, in application order.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(JoinOrder),
        Box::new(Cascade),
        Box::new(SemiJoin),
        Box::new(CheapFirst),
        Box::new(Fences),
    ]
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
    let mut alternatives = vec![];
    let mut plan_space = None;
    let mut take = |root: &mut CallNode, rw: Rewrite, name: &str| -> bool {
        let next = annotate(&annotate::to_logical(&rw.node), catalog, cost);
        let changed = annotate::to_logical(&next) != annotate::to_logical(root);
        if !rw.alternatives.is_empty() {
            alternatives = rw.alternatives;
        }
        if rw.plan_space.is_some() {
            plan_space = rw.plan_space;
        }
        if changed {
            *root = next;
            applied.push(name.to_string());
        }
        changed
    };
    for r in rules.iter().filter(|r| r.once()) {
        if let Some(rw) = r.apply(&root, catalog, cost) {
            take(&mut root, rw, r.name());
        }
    }
    for _round in 0..8 {
        let mut changed = false;
        for r in rules.iter().filter(|r| !r.once()) {
            if let Some(rw) = r.apply(&root, catalog, cost) {
                changed |= take(&mut root, rw, r.name());
            }
        }
        if !changed {
            break;
        }
    }
    let fragment = annotate::fragment(&root);
    let total = root.total();
    if let Some(chosen) = alternatives.iter_mut().find(|a| a.chosen) {
        chosen.estimate = total;
    }
    let mut names = vec![];
    plan_call_names(logical, catalog, &mut names);
    let learned = cost.learned_notes(&names);
    CallPlan {
        root,
        fragment,
        total,
        rules_applied: applied,
        alternatives,
        plan_space,
        learned,
    }
}
