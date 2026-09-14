//! Rewrite rules.

use crate::annotate::{conjuncts, is_equijoin, predicate_cost, volatility_of_expr, CostModel};
use crate::{Alternative, CallNode, Estimate, PlanSpace, Rewrite, Rule};
use callgebra_core::{CallKind, Catalog, Field, Schema, Volatility};
use callgebra_sql::{BinaryOp, Expr, JoinKind, LogicalPlan};
use std::collections::HashMap;
use std::sync::Arc;

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

    fn apply(&self, plan: &CallNode, catalog: &Catalog, _cost: &CostModel) -> Option<Rewrite> {
        rewrite_first(plan, &mut |node| {
            let (predicate, rebuild): (&Expr, Box<dyn Fn(Expr) -> LogicalPlan>) = match &node.op {
                LogicalPlan::Filter { predicate, input } => {
                    let input = input.clone();
                    (
                        predicate,
                        Box::new(move |p| LogicalPlan::Filter {
                            input: input.clone(),
                            predicate: p,
                        }),
                    )
                }
                LogicalPlan::Join {
                    left,
                    right,
                    kind,
                    on: Some(on),
                    schema,
                } => {
                    let (left, right, kind, schema) =
                        (left.clone(), right.clone(), *kind, schema.clone());
                    (
                        on,
                        Box::new(move |p| LogicalPlan::Join {
                            left: left.clone(),
                            right: right.clone(),
                            kind,
                            on: Some(p),
                            schema: schema.clone(),
                        }),
                    )
                }
                _ => return None,
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
                op: rebuild(and_all(ordered)),
                ..node.clone()
            })
        })
        .map(Rewrite::node)
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

    fn apply(&self, plan: &CallNode, _catalog: &Catalog, _cost: &CostModel) -> Option<Rewrite> {
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
        .map(Rewrite::node)
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

    fn apply(&self, _plan: &CallNode, _catalog: &Catalog, _cost: &CostModel) -> Option<Rewrite> {
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

/// Rule 4, cascade: an oracle predicate with a declared proxy becomes
/// `proxy >= high OR (proxy >= low AND oracle)`, so only the uncertain band
/// pays for the expensive call. The proxy is computed once per row in a
/// projection below the filter and dropped again above it.
pub struct Cascade;

impl Rule for Cascade {
    fn name(&self) -> &'static str {
        "cascade"
    }

    fn apply(&self, plan: &CallNode, catalog: &Catalog, cost: &CostModel) -> Option<Rewrite> {
        rewrite_first(plan, &mut |node| {
            let LogicalPlan::Filter { predicate, input } = &node.op else {
                return None;
            };
            if volatility_of_expr(predicate, catalog) == Volatility::Volatile {
                return None;
            }
            let parts = conjuncts(predicate);
            // The first conjunct that is a bare oracle call with a proxy.
            let (idx, fname, args) = parts.iter().enumerate().find_map(|(i, p)| match p {
                Expr::Function { name, args, .. } if catalog.proxy(name).is_some() => {
                    Some((i, name.clone(), args.clone()))
                }
                _ => None,
            })?;
            let spec = catalog.proxy(&fname)?.clone();
            let pdef = catalog.function(&spec.function)?;
            // Worth it only when the proxy tier is cheap enough that scoring
            // every row plus the oracle on the band beats the oracle alone.
            let odef = catalog.function(&fname)?;
            let in_rows = node.children.first()?.estimate.rows;
            let plain = cost
                .calls_for(std::slice::from_ref(&odef.call_kind), in_rows)
                .dollars;
            let cascaded = cost
                .calls_for(std::slice::from_ref(&pdef.call_kind), in_rows)
                .dollars
                + cost
                    .calls_for(
                        std::slice::from_ref(&odef.call_kind),
                        in_rows * (spec.high - spec.low),
                    )
                    .dollars;
            if cascaded >= plain {
                return None;
            }
            let in_schema = input.schema();
            let width = in_schema.len();
            // Project: every input column plus the proxy score.
            let mut exprs: Vec<Expr> = in_schema
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| Expr::Column {
                    index: i,
                    name: f.name.clone(),
                })
                .collect();
            let score_name = format!("__proxy_{}", fname.to_ascii_lowercase());
            exprs.push(Expr::Function {
                name: spec.function.clone(),
                args: args.clone(),
                data_type: match pdef.returns {
                    callgebra_core::FunctionReturn::Scalar { data_type } => data_type,
                    _ => callgebra_core::DataType::Float,
                },
            });
            let mut fields = in_schema.fields.clone();
            fields.push(Field::new(&score_name, callgebra_core::DataType::Float));
            let wide = Arc::new(Schema::new(fields));
            let project = LogicalPlan::Project {
                input: input.clone(),
                exprs,
                schema: wide.clone(),
            };
            let score = Expr::Column {
                index: width,
                name: score_name,
            };
            let lit =
                |v: f64| Expr::Literal(callgebra_sql::Literal(callgebra_core::Value::Float(v)));
            let cascade = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(Expr::Binary {
                    op: BinaryOp::GtEq,
                    left: Box::new(score.clone()),
                    right: Box::new(lit(spec.high)),
                }),
                right: Box::new(Expr::Binary {
                    op: BinaryOp::And,
                    left: Box::new(Expr::Binary {
                        op: BinaryOp::GtEq,
                        left: Box::new(score),
                        right: Box::new(lit(spec.low)),
                    }),
                    right: Box::new(parts[idx].clone()),
                }),
            };
            let mut new_parts = parts.clone();
            new_parts[idx] = cascade;
            let filter = LogicalPlan::Filter {
                input: Box::new(project.clone()),
                predicate: and_all(new_parts),
            };
            // Drop the score again.
            let restore = LogicalPlan::Project {
                input: Box::new(filter.clone()),
                exprs: (0..width)
                    .map(|i| Expr::Column {
                        index: i,
                        name: in_schema.fields[i].name.clone(),
                    })
                    .collect(),
                schema: in_schema.clone(),
            };
            let leaf = |op: LogicalPlan, children: Vec<CallNode>| CallNode {
                op,
                calls: vec![],
                volatility: node.volatility,
                estimate: Estimate::default(),
                children,
            };
            let input_node = node.children.first()?.clone();
            let project_node = leaf(project, vec![input_node]);
            let filter_node = leaf(filter, vec![project_node]);
            Some(leaf(restore, vec![filter_node]))
        })
        .map(Rewrite::node)
    }
}

/// Rule 7, join ordering with call predicates. Collects the relations under a
/// tree of inner and cross joins (with the filter above it), then runs
/// dynamic programming over subsets: every split of a subset is priced with
/// the predicates that first become applicable there, pure conjuncts first,
/// call conjuncts paying per surviving pair. Splits whose inputs already cost
/// more than the best plan for the subset are pruned. Single-relation
/// predicates are pushed to their scans. Runs once.
pub struct JoinOrder;

/// Largest join to enumerate exhaustively.
const MAX_RELATIONS: usize = 10;

struct Leaf {
    node: CallNode,
    offset: usize,
    width: usize,
}

struct Pred {
    expr: Expr,
    /// Bitmask of relations referenced.
    rels: u32,
    is_call: bool,
}

#[derive(Clone)]
struct Best {
    cost: Estimate,
    tree: CallNode,
    /// Relation order in the tree's output.
    order: Vec<usize>,
    label: String,
}

fn rank(e: &Estimate) -> (f64, f64) {
    (e.dollars, e.rows)
}

impl Rule for JoinOrder {
    fn name(&self) -> &'static str {
        "join-order"
    }

    fn once(&self) -> bool {
        true
    }

    fn apply(&self, plan: &CallNode, catalog: &Catalog, cost: &CostModel) -> Option<Rewrite> {
        let mut report = None;
        let node = rewrite_first(plan, &mut |node| {
            let rw = self.reorder(node, catalog, cost)?;
            report = Some((rw.alternatives, rw.plan_space));
            Some(rw.node)
        })?;
        let (alternatives, plan_space) = report.unwrap_or_default();
        Some(Rewrite {
            node,
            alternatives,
            plan_space,
        })
    }
}

impl JoinOrder {
    fn reorder(&self, node: &CallNode, catalog: &Catalog, cost: &CostModel) -> Option<Rewrite> {
        // Filter over joins, or bare joins.
        let (filter_pred, join_root) = match &node.op {
            LogicalPlan::Filter { predicate, .. } => {
                (Some(predicate.clone()), node.children.first()?)
            }
            LogicalPlan::Join {
                kind: JoinKind::Inner | JoinKind::Cross,
                ..
            } => (None, node),
            _ => return None,
        };
        if !matches!(
            join_root.op,
            LogicalPlan::Join {
                kind: JoinKind::Inner | JoinKind::Cross,
                ..
            }
        ) {
            return None;
        }
        let mut leaves: Vec<Leaf> = vec![];
        let mut preds: Vec<Expr> = vec![];
        collect(join_root, 0, &mut leaves, &mut preds);
        if leaves.len() < 3 || leaves.len() > MAX_RELATIONS {
            return None;
        }
        if let Some(p) = &filter_pred {
            preds.extend(conjuncts(p));
        }
        let total_width: usize = leaves.iter().map(|l| l.width).sum();
        let rel_of = |index: usize| -> Option<usize> {
            leaves
                .iter()
                .position(|l| index >= l.offset && index < l.offset + l.width)
        };
        let mut predicates = vec![];
        for e in preds {
            if volatility_of_expr(&e, catalog) == Volatility::Volatile || !movable(&e) {
                return None;
            }
            let mut rels = 0u32;
            for idx in columns(&e) {
                rels |= 1 << rel_of(idx)?;
            }
            let mut kinds = vec![];
            crate::annotate::calls_in_expr(&e, catalog, &mut kinds);
            predicates.push(Pred {
                expr: e,
                rels,
                is_call: !kinds.is_empty(),
            });
        }
        if leaves
            .iter()
            .any(|l| l.node.subtree_volatility() == Volatility::Volatile)
        {
            return None;
        }
        if !predicates.iter().any(|p| p.is_call) {
            // Nothing call-shaped to optimise; leave ordinary joins alone.
            return None;
        }
        let n = leaves.len();
        let full = (1u32 << n) - 1;
        // Price the plan as written for comparison.
        let written = node.total();
        let mut best: HashMap<u32, Best> = HashMap::new();
        let mut evaluated = 0u64;
        let mut pruned = 0u64;
        // Singletons: the leaf plus its own predicates.
        for (i, leaf) in leaves.iter().enumerate() {
            let mask = 1u32 << i;
            let own: Vec<&Pred> = predicates.iter().filter(|p| p.rels == mask).collect();
            let (tree, cost_est) =
                apply_preds(leaf.node.clone(), &own, &[i], &leaves, catalog, cost, false);
            best.insert(
                mask,
                Best {
                    cost: cost_est,
                    tree,
                    order: vec![i],
                    label: leaf_label(&leaf.node),
                },
            );
        }
        for size in 2..=n {
            for s in 1..=full {
                if s.count_ones() as usize != size {
                    continue;
                }
                let mut best_here: Option<Best> = None;
                // Enumerate splits: left = proper nonempty subset a of s.
                let mut a = (s - 1) & s;
                while a > 0 {
                    let b = s & !a;
                    if let (Some(la), Some(lb)) = (best.get(&a), best.get(&b)) {
                        let inputs = la.cost.plus(lb.cost);
                        if let Some(bh) = &best_here {
                            if rank(&inputs) >= rank(&bh.cost) {
                                pruned += 1;
                                a = (a - 1) & s;
                                continue;
                            }
                        }
                        evaluated += 1;
                        let new_preds: Vec<&Pred> = predicates
                            .iter()
                            .filter(|p| p.rels & !s == 0 && p.rels & a != 0 && p.rels & b != 0)
                            .collect();
                        let mut order = la.order.clone();
                        order.extend(lb.order.iter().copied());
                        let join = CallNode {
                            op: LogicalPlan::Join {
                                left: Box::new(la.tree.op.clone()),
                                right: Box::new(lb.tree.op.clone()),
                                kind: JoinKind::Cross,
                                on: None,
                                schema: Arc::new(schema_for(&order, &leaves)),
                            },
                            calls: vec![],
                            volatility: Volatility::Immutable,
                            estimate: Estimate {
                                rows: la.cost.rows * lb.cost.rows,
                                ..Estimate::default()
                            },
                            children: vec![la.tree.clone(), lb.tree.clone()],
                        };
                        let (tree, step) =
                            apply_preds(join, &new_preds, &order, &leaves, catalog, cost, true);
                        let mut total = inputs.plus(step);
                        total.rows = step.rows;
                        let candidate = Best {
                            cost: total,
                            tree,
                            order,
                            label: format!("({} ⋈ {})", la.label, lb.label),
                        };
                        if best_here
                            .as_ref()
                            .is_none_or(|bh| rank(&candidate.cost) < rank(&bh.cost))
                        {
                            best_here = Some(candidate);
                        }
                    }
                    a = (a - 1) & s;
                }
                if let Some(b) = best_here {
                    best.insert(s, b);
                }
            }
        }
        let chosen = best.remove(&full)?;
        // Predicates over no relation at all (constants) go on top.
        let consts: Vec<&Pred> = predicates.iter().filter(|p| p.rels == 0).collect();
        let (mut tree, _) = apply_preds(
            chosen.tree,
            &consts,
            &chosen.order,
            &leaves,
            catalog,
            cost,
            false,
        );
        // Restore the column order the query was written in.
        let mut new_index = vec![0usize; total_width];
        let mut pos = 0;
        for &r in &chosen.order {
            for k in 0..leaves[r].width {
                new_index[leaves[r].offset + k] = pos;
                pos += 1;
            }
        }
        let out_schema = join_root.op.schema();
        if new_index.iter().enumerate().any(|(i, j)| i != *j) {
            tree = CallNode {
                op: LogicalPlan::Project {
                    input: Box::new(tree.op.clone()),
                    exprs: (0..total_width)
                        .map(|i| Expr::Column {
                            index: new_index[i],
                            name: out_schema.fields[i].name.clone(),
                        })
                        .collect(),
                    schema: out_schema.clone(),
                },
                calls: vec![],
                volatility: Volatility::Immutable,
                estimate: Estimate::default(),
                children: vec![tree],
            };
        }
        let orders = bushy_orders(n as u64);
        let alternatives = vec![
            Alternative {
                label: "as written".into(),
                estimate: written,
                chosen: false,
            },
            Alternative {
                label: format!("chosen: {}", chosen.label),
                estimate: chosen.cost,
                chosen: true,
            },
        ];
        Some(Rewrite {
            node: tree,
            alternatives,
            plan_space: Some(PlanSpace {
                relations: n,
                orders,
                evaluated,
                pruned,
            }),
        })
    }
}

/// Number of bushy join trees over `n` labelled relations: `n! × C(n-1)`.
fn bushy_orders(n: u64) -> u64 {
    let fact: u64 = (1..=n).product();
    let catalan = (0..n.saturating_sub(1)).fold(1u64, |c, i| c * 2 * (2 * i + 1) / (i + 2));
    fact.saturating_mul(catalan)
}

fn leaf_label(n: &CallNode) -> String {
    match &n.op {
        LogicalPlan::Scan { table, .. } => table.clone(),
        LogicalPlan::CteRef { name, .. } => name.clone(),
        LogicalPlan::Values { .. } => "values".into(),
        LogicalPlan::Filter { input, .. } => match &**input {
            LogicalPlan::Scan { table, .. } => format!("σ{table}"),
            _ => "σ(…)".into(),
        },
        _ => "(…)".into(),
    }
}

/// Walk a tree of inner/cross joins, collecting leaves with their column
/// offsets in the as-written output and every `on` conjunct in that space.
fn collect(node: &CallNode, offset: usize, leaves: &mut Vec<Leaf>, preds: &mut Vec<Expr>) -> usize {
    match &node.op {
        LogicalPlan::Join {
            kind: JoinKind::Inner | JoinKind::Cross,
            on,
            ..
        } if node.children.len() == 2 => {
            let lw = collect(&node.children[0], offset, leaves, preds);
            let rw = collect(&node.children[1], offset + lw, leaves, preds);
            if let Some(p) = on {
                for c in conjuncts(p) {
                    preds.push(shift_columns(&c, offset));
                }
            }
            lw + rw
        }
        _ => {
            let width = node.op.schema().len();
            leaves.push(Leaf {
                node: node.clone(),
                offset,
                width,
            });
            width
        }
    }
}

/// Column indices referenced by an expression.
fn columns(e: &Expr) -> Vec<usize> {
    let mut out = vec![];
    fn walk(e: &Expr, out: &mut Vec<usize>) {
        match e {
            Expr::Column { index, .. } => out.push(*index),
            Expr::Binary { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => walk(operand, out),
            Expr::Function { args, .. } | Expr::Aggregate { args, .. } => {
                for a in args {
                    walk(a, out);
                }
            }
            Expr::Case {
                branches,
                otherwise,
            } => {
                for (c, r) in branches {
                    walk(c, out);
                    walk(r, out);
                }
                if let Some(o) = otherwise {
                    walk(o, out);
                }
            }
            Expr::InList { operand, list, .. } => {
                walk(operand, out);
                for l in list {
                    walk(l, out);
                }
            }
            Expr::InSubquery { operand, .. } => walk(operand, out),
            Expr::Literal(_)
            | Expr::OuterColumn { .. }
            | Expr::Exists { .. }
            | Expr::ScalarSubquery { .. } => {}
        }
    }
    walk(e, &mut out);
    out
}

/// Predicates the reorderer knows how to move: no subqueries, no outer
/// references, no aggregates.
fn movable(e: &Expr) -> bool {
    match e {
        Expr::Exists { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::InSubquery { .. }
        | Expr::OuterColumn { .. }
        | Expr::Aggregate { .. } => false,
        Expr::Column { .. } | Expr::Literal(_) => true,
        Expr::Binary { left, right, .. } => movable(left) && movable(right),
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => movable(operand),
        Expr::Function { args, .. } => args.iter().all(movable),
        Expr::Case {
            branches,
            otherwise,
        } => {
            branches.iter().all(|(c, r)| movable(c) && movable(r))
                && otherwise.as_deref().is_none_or(movable)
        }
        Expr::InList { operand, list, .. } => movable(operand) && list.iter().all(movable),
    }
}

/// Add `delta` to every column index.
fn shift_columns(e: &Expr, delta: usize) -> Expr {
    remap_columns(e, &|i| i + delta)
}

fn remap_columns(e: &Expr, f: &dyn Fn(usize) -> usize) -> Expr {
    match e {
        Expr::Column { index, name } => Expr::Column {
            index: f(*index),
            name: name.clone(),
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op: *op,
            left: Box::new(remap_columns(left, f)),
            right: Box::new(remap_columns(right, f)),
        },
        Expr::Unary { op, operand } => Expr::Unary {
            op: *op,
            operand: Box::new(remap_columns(operand, f)),
        },
        Expr::Cast { operand, to } => Expr::Cast {
            operand: Box::new(remap_columns(operand, f)),
            to: *to,
        },
        Expr::Function {
            name,
            args,
            data_type,
        } => Expr::Function {
            name: name.clone(),
            args: args.iter().map(|a| remap_columns(a, f)).collect(),
            data_type: *data_type,
        },
        Expr::Aggregate {
            func,
            args,
            distinct,
        } => Expr::Aggregate {
            func: *func,
            args: args.iter().map(|a| remap_columns(a, f)).collect(),
            distinct: *distinct,
        },
        Expr::Case {
            branches,
            otherwise,
        } => Expr::Case {
            branches: branches
                .iter()
                .map(|(c, r)| (remap_columns(c, f), remap_columns(r, f)))
                .collect(),
            otherwise: otherwise.as_ref().map(|o| Box::new(remap_columns(o, f))),
        },
        Expr::InList {
            operand,
            list,
            negated,
        } => Expr::InList {
            operand: Box::new(remap_columns(operand, f)),
            list: list.iter().map(|a| remap_columns(a, f)).collect(),
            negated: *negated,
        },
        other => other.clone(),
    }
}

/// Output schema of a join producing the relations in `order`.
fn schema_for(order: &[usize], leaves: &[Leaf]) -> Schema {
    let mut fields = vec![];
    for &r in order {
        fields.extend(leaves[r].node.op.schema().fields.iter().cloned());
    }
    Schema::new(fields)
}

/// Position of an as-written column in the output of a tree over `order`.
fn position(index: usize, order: &[usize], leaves: &[Leaf]) -> usize {
    let mut pos = 0;
    for &r in order {
        let l = &leaves[r];
        if index >= l.offset && index < l.offset + l.width {
            return pos + (index - l.offset);
        }
        pos += l.width;
    }
    index
}

/// Attach predicates to a tree (as the join's `on`, or as a filter over a
/// leaf), pure conjuncts first, and price the result. Returns the tree and
/// the estimate of this step (rows = survivors, calls = this step's calls).
fn apply_preds(
    tree: CallNode,
    preds: &[&Pred],
    order: &[usize],
    leaves: &[Leaf],
    catalog: &Catalog,
    cost: &CostModel,
    is_join: bool,
) -> (CallNode, Estimate) {
    let in_rows = tree.estimate.rows;
    if preds.is_empty() {
        return (
            tree,
            Estimate {
                rows: in_rows,
                ..Estimate::default()
            },
        );
    }
    let mut ordered: Vec<Expr> = preds
        .iter()
        .filter(|p| !p.is_call)
        .map(|p| remap_columns(&p.expr, &|i| position(i, order, leaves)))
        .collect();
    // Equijoins first among the pure ones, then the rest, then calls.
    ordered.sort_by_key(|e| if is_equijoin(e) { 0 } else { 1 });
    ordered.extend(
        preds
            .iter()
            .filter(|p| p.is_call)
            .map(|p| remap_columns(&p.expr, &|i| position(i, order, leaves))),
    );
    let predicate = and_all(ordered);
    let (est, calls) = predicate_cost(&predicate, catalog, cost, in_rows, is_join);
    let volatility = volatility_of_expr(&predicate, catalog);
    let node = match (&tree.op, is_join) {
        (
            LogicalPlan::Join {
                left,
                right,
                schema,
                ..
            },
            true,
        ) => CallNode {
            op: LogicalPlan::Join {
                left: left.clone(),
                right: right.clone(),
                kind: JoinKind::Inner,
                on: Some(predicate),
                schema: schema.clone(),
            },
            calls,
            volatility,
            estimate: est,
            children: tree.children.clone(),
        },
        _ => CallNode {
            op: LogicalPlan::Filter {
                input: Box::new(tree.op.clone()),
                predicate,
            },
            calls,
            volatility,
            estimate: est,
            children: vec![tree],
        },
    };
    (node, est)
}
