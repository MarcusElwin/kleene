//! Rewrite rules.

use crate::annotate::volatility_of_expr;
use crate::{CallNode, Rule};
use callgebra_core::{CallKind, Catalog, Volatility};
use callgebra_sql::{BinaryOp, Expr, JoinKind, LogicalPlan};
use std::sync::Arc;

fn conjuncts(e: &Expr) -> Vec<Expr> {
    match e {
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
        } => {
            let mut v = conjuncts(left);
            v.extend(conjuncts(right));
            v
        }
        other => vec![other.clone()],
    }
}

fn and_all(mut parts: Vec<Expr>) -> Expr {
    let mut acc = parts.remove(0);
    for p in parts {
        acc = Expr::Binary {
            op: BinaryOp::And,
            left: Box::new(acc),
            right: Box::new(p),
        };
    }
    acc
}

/// Number of model calls in an expression (0 for pure ones).
fn call_weight(e: &Expr, catalog: &Catalog) -> usize {
    let mut calls = vec![];
    crate::annotate::calls_in_expr(e, catalog, &mut calls);
    calls
        .iter()
        .map(|c| match c {
            CallKind::Pure => 0,
            CallKind::Tool { .. } => 1,
            CallKind::LlmScalar { .. } | CallKind::LlmTable { .. } => 10,
            CallKind::Recursive { .. } => 100,
        })
        .sum::<usize>()
        + if has_subquery(e) { 5 } else { 0 }
}

fn has_subquery(e: &Expr) -> bool {
    let mut subs = vec![];
    crate::annotate::subqueries_in_expr(e, &mut subs);
    !subs.is_empty()
}

/// Rule 1: within a conjunctive filter, evaluate cheap predicates before
/// call predicates so `AND` short-circuits away model calls. Never moves a
/// conjunct across a `VOLATILE` one (rule 8 is enforced here too).
pub struct CheapFirst;

impl Rule for CheapFirst {
    fn name(&self) -> &'static str {
        "cheap-first"
    }

    fn apply(&self, plan: &CallNode, catalog: &Catalog) -> Option<CallNode> {
        rewrite_first(plan, &mut |node| {
            let LogicalPlan::Filter { predicate, input } = &node.op else {
                return None;
            };
            let parts = conjuncts(predicate);
            if parts.len() < 2 {
                return None;
            }
            // Stable sort by weight within fenced segments.
            let mut segments: Vec<Vec<Expr>> = vec![vec![]];
            for p in &parts {
                if volatility_of_expr(p, catalog) == Volatility::Volatile {
                    segments.push(vec![p.clone()]);
                    segments.push(vec![]);
                } else {
                    segments.last_mut().expect("segment").push(p.clone());
                }
            }
            let mut ordered: Vec<Expr> = vec![];
            for seg in segments {
                let mut s = seg;
                s.sort_by_key(|e| call_weight(e, catalog));
                ordered.extend(s);
            }
            if ordered == parts {
                return None;
            }
            Some(CallNode {
                op: LogicalPlan::Filter {
                    input: input.clone(),
                    predicate: and_all(ordered),
                },
                ..node.clone()
            })
        })
    }
}

/// Rule 5: `[NOT] EXISTS (SELECT ... FROM R WHERE p)` where `p` correlates
/// only with the immediately enclosing row becomes a semi- or anti-join
/// against `R`, which stops at the first (counter)example instead of
/// materialising the cross product. Only applies when the subquery is a
/// filter (optionally under a projection) over a single relation with no
/// deeper outer references.
pub struct SemiJoin;

impl Rule for SemiJoin {
    fn name(&self) -> &'static str {
        "semi-join"
    }

    fn apply(&self, plan: &CallNode, _catalog: &Catalog) -> Option<CallNode> {
        rewrite_first(plan, &mut |node| {
            let LogicalPlan::Filter { predicate, .. } = &node.op else {
                return None;
            };
            let parts = conjuncts(predicate);
            let idx = parts
                .iter()
                .position(|p| matches!(p, Expr::Exists { .. }))?;
            let Expr::Exists { subquery, negated } = &parts[idx] else {
                return None;
            };
            let (right_plan, pred) = decorrelate(subquery)?;
            let left_child = node.children.first()?.clone();
            let left_width = left_child.op.schema().len();
            let right_width = right_plan.schema().len();
            let mut left_fields = left_child.op.schema().fields.clone();
            left_fields.extend(right_plan.schema().fields.iter().cloned());
            let join_schema = Arc::new(callgebra_core::Schema::new(left_fields));
            let on = shift_outer(&pred, left_width)?;
            let _ = right_width;
            let join = LogicalPlan::Join {
                left: Box::new(left_child.op.clone()),
                right: Box::new(right_plan.clone()),
                kind: if *negated {
                    JoinKind::Anti
                } else {
                    JoinKind::Semi
                },
                on: Some(on),
                schema: Arc::new(callgebra_core::Schema::new(
                    join_schema.fields[..left_width].to_vec(),
                )),
            };
            let right_node = CallNode {
                op: right_plan.clone(),
                calls: vec![],
                volatility: Volatility::Stable,
                estimate: Default::default(),
                children: vec![],
            };
            let join_node = CallNode {
                op: join,
                calls: vec![],
                volatility: node.volatility,
                estimate: Default::default(),
                children: vec![left_child, right_node],
            };
            let rest: Vec<Expr> = parts
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .map(|(_, e)| e.clone())
                .collect();
            if rest.is_empty() {
                Some(join_node)
            } else {
                Some(CallNode {
                    op: LogicalPlan::Filter {
                        input: Box::new(join_node.op.clone()),
                        predicate: and_all(rest),
                    },
                    calls: vec![],
                    volatility: node.volatility,
                    estimate: Default::default(),
                    children: vec![join_node],
                })
            }
        })
    }
}

/// If `sub` is `Project(Filter(R, p))` or `Filter(R, p)` where `R` has no
/// outer references, return `(R, p)`.
fn decorrelate(sub: &LogicalPlan) -> Option<(LogicalPlan, Expr)> {
    let inner = match sub {
        LogicalPlan::Project { input, .. } => &**input,
        other => other,
    };
    let LogicalPlan::Filter { input, predicate } = inner else {
        return None;
    };
    if has_outer_refs(input) || max_outer_depth(predicate) > 1 || has_subquery(predicate) {
        return None;
    }
    if !matches!(
        **input,
        LogicalPlan::Scan { .. } | LogicalPlan::CteRef { .. } | LogicalPlan::Values { .. }
    ) {
        return None;
    }
    Some(((**input).clone(), predicate.clone()))
}

fn has_outer_refs(p: &LogicalPlan) -> bool {
    fn expr_has(e: &Expr) -> bool {
        max_outer_depth(e) > 0
    }
    match p {
        LogicalPlan::Filter { predicate, input } => expr_has(predicate) || has_outer_refs(input),
        LogicalPlan::Project { exprs, input, .. } => {
            exprs.iter().any(expr_has) || has_outer_refs(input)
        }
        LogicalPlan::Join {
            on, left, right, ..
        } => on.as_ref().is_some_and(expr_has) || has_outer_refs(left) || has_outer_refs(right),
        LogicalPlan::TableFunction { args, input, .. } => {
            args.iter().any(expr_has) || input.as_deref().is_some_and(has_outer_refs)
        }
        other => other.children().into_iter().any(has_outer_refs),
    }
}

fn max_outer_depth(e: &Expr) -> usize {
    match e {
        Expr::OuterColumn { depth, .. } => *depth,
        Expr::Binary { left, right, .. } => max_outer_depth(left).max(max_outer_depth(right)),
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => max_outer_depth(operand),
        Expr::Function { args, .. } | Expr::Aggregate { args, .. } => {
            args.iter().map(max_outer_depth).max().unwrap_or(0)
        }
        Expr::Case {
            branches,
            otherwise,
        } => branches
            .iter()
            .map(|(c, r)| max_outer_depth(c).max(max_outer_depth(r)))
            .chain(otherwise.iter().map(|o| max_outer_depth(o)))
            .max()
            .unwrap_or(0),
        Expr::InList { operand, list, .. } => list
            .iter()
            .map(max_outer_depth)
            .max()
            .unwrap_or(0)
            .max(max_outer_depth(operand)),
        Expr::InSubquery { operand, .. } => max_outer_depth(operand).max(1),
        Expr::Exists { .. } | Expr::ScalarSubquery { .. } => 1,
        Expr::Column { .. } | Expr::Literal(_) => 0,
    }
}

/// Rewrite a subquery predicate into a join predicate over `left ++ right`:
/// `OuterColumn{1, i}` becomes `Column{i}`, `Column{j}` becomes
/// `Column{left_width + j}`.
fn shift_outer(e: &Expr, left_width: usize) -> Option<Expr> {
    Some(match e {
        Expr::OuterColumn {
            depth: 1,
            index,
            name,
        } => Expr::Column {
            index: *index,
            name: name.clone(),
        },
        Expr::OuterColumn { .. } => return None,
        Expr::Column { index, name } => Expr::Column {
            index: left_width + index,
            name: name.clone(),
        },
        Expr::Literal(l) => Expr::Literal(l.clone()),
        Expr::Binary { op, left, right } => Expr::Binary {
            op: *op,
            left: Box::new(shift_outer(left, left_width)?),
            right: Box::new(shift_outer(right, left_width)?),
        },
        Expr::Unary { op, operand } => Expr::Unary {
            op: *op,
            operand: Box::new(shift_outer(operand, left_width)?),
        },
        Expr::Cast { operand, to } => Expr::Cast {
            operand: Box::new(shift_outer(operand, left_width)?),
            to: *to,
        },
        Expr::Function {
            name,
            args,
            data_type,
        } => Expr::Function {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| shift_outer(a, left_width))
                .collect::<Option<_>>()?,
            data_type: *data_type,
        },
        Expr::Case {
            branches,
            otherwise,
        } => Expr::Case {
            branches: branches
                .iter()
                .map(|(c, r)| Some((shift_outer(c, left_width)?, shift_outer(r, left_width)?)))
                .collect::<Option<_>>()?,
            otherwise: match otherwise {
                Some(o) => Some(Box::new(shift_outer(o, left_width)?)),
                None => None,
            },
        },
        Expr::InList {
            operand,
            list,
            negated,
        } => Expr::InList {
            operand: Box::new(shift_outer(operand, left_width)?),
            list: list
                .iter()
                .map(|a| shift_outer(a, left_width))
                .collect::<Option<_>>()?,
            negated: *negated,
        },
        Expr::Aggregate { .. }
        | Expr::Exists { .. }
        | Expr::InSubquery { .. }
        | Expr::ScalarSubquery { .. } => return None,
    })
}

/// Rule 8 as a check: a `VOLATILE` operator is a fence. This rule never
/// changes the plan; it exists so `EXPLAIN` can report fences, and so the
/// rule list documents the invariant the other rules respect.
pub struct Fences;

impl Rule for Fences {
    fn name(&self) -> &'static str {
        "fences"
    }

    fn apply(&self, _plan: &CallNode, _catalog: &Catalog) -> Option<CallNode> {
        None
    }
}

/// Apply `f` to the first node (pre-order) where it returns `Some`.
fn rewrite_first(
    node: &CallNode,
    f: &mut dyn FnMut(&CallNode) -> Option<CallNode>,
) -> Option<CallNode> {
    if let Some(n) = f(node) {
        return Some(n);
    }
    for (i, c) in node.children.iter().enumerate() {
        if let Some(nc) = rewrite_first(c, f) {
            let mut out = node.clone();
            out.children[i] = nc;
            return Some(out);
        }
    }
    None
}
