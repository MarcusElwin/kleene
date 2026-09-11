//! Annotate a logical plan with call kinds, volatility and estimates.

use crate::{CallNode, Estimate, Fragment};
use callgebra_core::{CallKind, Catalog, FunctionDef, Volatility};
use callgebra_sql::{Expr, JoinKind, LogicalPlan};
use std::collections::HashMap;

/// Parameters of the cost model. Everything is a guess in M2; M5 learns
/// selectivities and branching factors from `EXPLAIN ANALYZE` actuals.
#[derive(Debug, Clone, PartialEq)]
pub struct CostModel {
    /// Known row counts per stored table (from the store), lower case names.
    pub table_rows: HashMap<String, f64>,
    /// Rows assumed for a table with no known count.
    pub default_rows: f64,
    /// Fraction of rows a filter keeps.
    pub filter_selectivity: f64,
    /// Fraction of `left × right` an inner join keeps.
    pub join_selectivity: f64,
    /// Rows a table call produces per input row (`EXPAND`, tools).
    pub branching: f64,
    /// Rounds assumed for a recursive query.
    pub recursion_rounds: f64,
    /// Input tokens per model call.
    pub tokens_in_per_call: f64,
    /// Output tokens per model call.
    pub tokens_out_per_call: f64,
    /// Dollars per input token.
    pub usd_per_input_token: f64,
    /// Dollars per output token.
    pub usd_per_output_token: f64,
    /// Fraction of call inputs expected to be distinct (dedupe rule).
    pub distinct_fraction: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            table_rows: HashMap::new(),
            default_rows: 100.0,
            filter_selectivity: 0.5,
            join_selectivity: 0.1,
            branching: 3.0,
            recursion_rounds: 4.0,
            tokens_in_per_call: 600.0,
            tokens_out_per_call: 150.0,
            usd_per_input_token: 2e-6,
            usd_per_output_token: 1e-5,
            distinct_fraction: 1.0,
        }
    }
}

impl CostModel {
    /// Cost of `n` model calls.
    fn calls(&self, n: f64) -> Estimate {
        let tokens = n * (self.tokens_in_per_call + self.tokens_out_per_call);
        let dollars = n
            * (self.tokens_in_per_call * self.usd_per_input_token
                + self.tokens_out_per_call * self.usd_per_output_token);
        Estimate {
            rows: 0.0,
            calls: n,
            tokens,
            dollars,
            depth: 0,
        }
    }
}

/// Call kinds of every function referenced in an expression, catalog order
/// ignored (evaluation order within one expression is not modelled).
pub(crate) fn calls_in_expr(e: &Expr, catalog: &Catalog, out: &mut Vec<CallKind>) {
    match e {
        Expr::Function { name, args, .. } => {
            for a in args {
                calls_in_expr(a, catalog, out);
            }
            if let Some(def) = catalog.function(name) {
                if def.call_kind != CallKind::Pure {
                    out.push(def.call_kind.clone());
                }
            }
        }
        Expr::Column { .. } | Expr::Literal(_) | Expr::OuterColumn { .. } => {}
        Expr::Binary { left, right, .. } => {
            calls_in_expr(left, catalog, out);
            calls_in_expr(right, catalog, out);
        }
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => {
            calls_in_expr(operand, catalog, out)
        }
        Expr::Aggregate { args, .. } => {
            for a in args {
                calls_in_expr(a, catalog, out);
            }
        }
        Expr::Case {
            branches,
            otherwise,
        } => {
            for (c, r) in branches {
                calls_in_expr(c, catalog, out);
                calls_in_expr(r, catalog, out);
            }
            if let Some(o) = otherwise {
                calls_in_expr(o, catalog, out);
            }
        }
        Expr::InList { operand, list, .. } => {
            calls_in_expr(operand, catalog, out);
            for l in list {
                calls_in_expr(l, catalog, out);
            }
        }
        Expr::InSubquery { operand, .. } => calls_in_expr(operand, catalog, out),
        Expr::Exists { .. } | Expr::ScalarSubquery { .. } => {}
    }
}

/// Subqueries nested in an expression.
pub(crate) fn subqueries_in_expr<'a>(e: &'a Expr, out: &mut Vec<&'a LogicalPlan>) {
    match e {
        Expr::Exists { subquery, .. } | Expr::ScalarSubquery { subquery } => out.push(subquery),
        Expr::InSubquery {
            operand, subquery, ..
        } => {
            subqueries_in_expr(operand, out);
            out.push(subquery);
        }
        Expr::Binary { left, right, .. } => {
            subqueries_in_expr(left, out);
            subqueries_in_expr(right, out);
        }
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => {
            subqueries_in_expr(operand, out)
        }
        Expr::Function { args, .. } | Expr::Aggregate { args, .. } => {
            for a in args {
                subqueries_in_expr(a, out);
            }
        }
        Expr::Case {
            branches,
            otherwise,
        } => {
            for (c, r) in branches {
                subqueries_in_expr(c, out);
                subqueries_in_expr(r, out);
            }
            if let Some(o) = otherwise {
                subqueries_in_expr(o, out);
            }
        }
        Expr::InList { operand, list, .. } => {
            subqueries_in_expr(operand, out);
            for l in list {
                subqueries_in_expr(l, out);
            }
        }
        Expr::Column { .. } | Expr::Literal(_) | Expr::OuterColumn { .. } => {}
    }
}

/// Volatility of everything in an expression.
pub(crate) fn volatility_of_expr(e: &Expr, catalog: &Catalog) -> Volatility {
    let mut v = Volatility::Immutable;
    let mut stack = vec![e];
    while let Some(x) = stack.pop() {
        match x {
            Expr::Function { name, args, .. } => {
                if let Some(def) = catalog.function(name) {
                    v = v.max(def.volatility);
                }
                stack.extend(args.iter());
            }
            Expr::Binary { left, right, .. } => {
                stack.push(left);
                stack.push(right);
            }
            Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => stack.push(operand),
            Expr::Aggregate { args, .. } => stack.extend(args.iter()),
            Expr::Case {
                branches,
                otherwise,
            } => {
                for (c, r) in branches {
                    stack.push(c);
                    stack.push(r);
                }
                if let Some(o) = otherwise {
                    stack.push(o);
                }
            }
            Expr::InList { operand, list, .. } => {
                stack.push(operand);
                stack.extend(list.iter());
            }
            Expr::InSubquery { operand, .. } => stack.push(operand),
            Expr::Exists { .. }
            | Expr::ScalarSubquery { .. }
            | Expr::Column { .. }
            | Expr::Literal(_)
            | Expr::OuterColumn { .. } => {}
        }
    }
    v
}

fn expr_list_calls(exprs: &[Expr], catalog: &Catalog) -> Vec<CallKind> {
    let mut out = vec![];
    for e in exprs {
        calls_in_expr(e, catalog, &mut out);
    }
    out
}

fn expr_list_volatility(exprs: &[Expr], catalog: &Catalog) -> Volatility {
    exprs
        .iter()
        .map(|e| volatility_of_expr(e, catalog))
        .fold(Volatility::Immutable, Volatility::max)
}

fn table_volatility(catalog: &Catalog, table: &str) -> Volatility {
    catalog
        .table(table)
        .map(|t| t.volatility)
        .unwrap_or(Volatility::Stable)
}

fn fn_def<'a>(catalog: &'a Catalog, name: &str) -> Option<&'a FunctionDef> {
    catalog.function(name)
}

/// Subquery costs are charged once per outer row.
fn subquery_cost(
    exprs: &[Expr],
    catalog: &Catalog,
    cost: &CostModel,
    outer_rows: f64,
) -> (Estimate, Vec<CallNode>) {
    let mut subs = vec![];
    for e in exprs {
        subqueries_in_expr(e, &mut subs);
    }
    let mut est = Estimate::default();
    let mut nodes = vec![];
    for s in subs {
        let node = annotate(s, catalog, cost);
        let per = node.total();
        est = est.plus(Estimate {
            rows: 0.0,
            calls: per.calls * outer_rows,
            tokens: per.tokens * outer_rows,
            dollars: per.dollars * outer_rows,
            depth: per.depth,
        });
        nodes.push(node);
    }
    (est, nodes)
}

/// Annotate a plan (children first).
pub fn annotate(plan: &LogicalPlan, catalog: &Catalog, cost: &CostModel) -> CallNode {
    let children: Vec<CallNode> = plan
        .children()
        .into_iter()
        .map(|c| annotate(c, catalog, cost))
        .collect();
    let in_rows: f64 = children.first().map(|c| c.estimate.rows).unwrap_or(1.0);
    let (calls, volatility, mut estimate) = match plan {
        LogicalPlan::Scan { table, .. } => {
            let rows = cost
                .table_rows
                .get(&table.to_ascii_lowercase())
                .copied()
                .unwrap_or(cost.default_rows);
            (
                vec![],
                table_volatility(catalog, table),
                Estimate {
                    rows,
                    ..Estimate::default()
                },
            )
        }
        LogicalPlan::Values { rows, .. } => (
            vec![],
            Volatility::Immutable,
            Estimate {
                rows: rows.len() as f64,
                ..Estimate::default()
            },
        ),
        LogicalPlan::CteRef { .. } => (
            vec![],
            Volatility::Stable,
            Estimate {
                rows: cost.default_rows,
                ..Estimate::default()
            },
        ),
        LogicalPlan::Project { exprs, .. } => {
            let calls = expr_list_calls(exprs, catalog);
            let n_calls = calls.len() as f64 * in_rows * cost.distinct_fraction;
            let mut est = cost.calls(n_calls);
            est.rows = in_rows;
            let (sub, _) = subquery_cost(exprs, catalog, cost, in_rows);
            (calls, expr_list_volatility(exprs, catalog), est.plus(sub))
        }
        LogicalPlan::Filter { predicate, .. } => {
            let exprs = std::slice::from_ref(predicate);
            let calls = expr_list_calls(exprs, catalog);
            let n_calls = calls.len() as f64 * in_rows * cost.distinct_fraction;
            let mut est = cost.calls(n_calls);
            est.rows = in_rows * cost.filter_selectivity;
            let (sub, _) = subquery_cost(exprs, catalog, cost, in_rows);
            (calls, expr_list_volatility(exprs, catalog), est.plus(sub))
        }
        LogicalPlan::Join { kind, on, .. } => {
            let l = children.first().map(|c| c.estimate.rows).unwrap_or(1.0);
            let r = children.get(1).map(|c| c.estimate.rows).unwrap_or(1.0);
            let pairs = l * r;
            let exprs: Vec<Expr> = on.iter().cloned().collect();
            let calls = expr_list_calls(&exprs, catalog);
            let mut est = cost.calls(calls.len() as f64 * pairs);
            est.rows = match kind {
                JoinKind::Cross => pairs,
                JoinKind::Inner => (pairs * cost.join_selectivity).max(1.0),
                JoinKind::Left => (pairs * cost.join_selectivity).max(l),
                JoinKind::Semi => l * cost.filter_selectivity,
                JoinKind::Anti => l * (1.0 - cost.filter_selectivity),
            };
            (calls, expr_list_volatility(&exprs, catalog), est)
        }
        LogicalPlan::TableFunction {
            name, args, input, ..
        } => {
            let def = fn_def(catalog, name);
            let per_input = if input.is_some() { in_rows } else { 1.0 };
            let kind = def.map(|d| d.call_kind.clone()).unwrap_or(CallKind::Pure);
            let vol = def.map(|d| d.volatility).unwrap_or(Volatility::Stable);
            let mut calls = expr_list_calls(args, catalog);
            let mut est = cost.calls(calls.len() as f64 * per_input);
            match &kind {
                CallKind::Pure => {}
                k => {
                    calls.push(k.clone());
                    if matches!(k, CallKind::LlmTable { .. } | CallKind::Recursive { .. }) {
                        est = est.plus(cost.calls(per_input));
                    }
                }
            }
            est.rows = per_input * cost.branching;
            if matches!(kind, CallKind::Recursive { .. }) {
                est.depth = 1;
            }
            (calls, vol.max(expr_list_volatility(args, catalog)), est)
        }
        LogicalPlan::Aggregate {
            group_by,
            aggregates,
            ..
        } => {
            let mut exprs = group_by.clone();
            exprs.extend(aggregates.iter().cloned());
            let calls = expr_list_calls(&exprs, catalog);
            let mut est = cost.calls(calls.len() as f64 * in_rows);
            est.rows = if group_by.is_empty() {
                1.0
            } else {
                (in_rows * 0.3).max(1.0)
            };
            (calls, expr_list_volatility(&exprs, catalog), est)
        }
        LogicalPlan::Sort { keys, .. } => {
            let exprs: Vec<Expr> = keys.iter().map(|k| k.expr.clone()).collect();
            let calls = expr_list_calls(&exprs, catalog);
            let mut est = cost.calls(calls.len() as f64 * in_rows);
            est.rows = in_rows;
            (calls, expr_list_volatility(&exprs, catalog), est)
        }
        LogicalPlan::Limit { limit, offset, .. } => (
            vec![],
            Volatility::Immutable,
            Estimate {
                rows: limit
                    .map(|l| l as f64)
                    .unwrap_or(in_rows)
                    .min((in_rows - *offset as f64).max(0.0)),
                ..Estimate::default()
            },
        ),
        LogicalPlan::Distinct { .. } => (
            vec![],
            Volatility::Immutable,
            Estimate {
                rows: (in_rows * 0.8).max(1.0),
                ..Estimate::default()
            },
        ),
        LogicalPlan::Union { all, .. } => {
            let rows: f64 = children.iter().map(|c| c.estimate.rows).sum();
            (
                vec![],
                Volatility::Immutable,
                Estimate {
                    rows: if *all { rows } else { rows * 0.8 },
                    ..Estimate::default()
                },
            )
        }
        LogicalPlan::With { .. } => (
            vec![],
            Volatility::Immutable,
            Estimate {
                rows: children.last().map(|c| c.estimate.rows).unwrap_or(in_rows),
                ..Estimate::default()
            },
        ),
        LogicalPlan::Recursive { .. } => {
            // children: base, recursive, body. The recursive term runs once per round.
            let rec = children.get(1).map(CallNode::total).unwrap_or_default();
            let rounds = cost.recursion_rounds;
            let body_rows = children.get(2).map(|c| c.estimate.rows).unwrap_or(in_rows);
            (
                vec![],
                Volatility::Immutable,
                Estimate {
                    rows: body_rows,
                    calls: rec.calls * (rounds - 1.0).max(0.0),
                    tokens: rec.tokens * (rounds - 1.0).max(0.0),
                    dollars: rec.dollars * (rounds - 1.0).max(0.0),
                    depth: 0,
                },
            )
        }
    };
    if let CallKind::Recursive { .. } = calls.last().cloned().unwrap_or(CallKind::Pure) {
        estimate.depth = estimate.depth.max(1);
    }
    CallNode {
        op: strip_children(plan),
        calls,
        volatility,
        estimate,
        children,
    }
}

/// The operator with its children replaced by empty placeholders (children
/// live in [`CallNode::children`]).
fn strip_children(plan: &LogicalPlan) -> LogicalPlan {
    plan.clone()
}

/// Rebuild the logical plan from a call node (children come from the node).
pub(crate) fn to_logical(node: &CallNode) -> LogicalPlan {
    let kids: Vec<LogicalPlan> = node.children.iter().map(to_logical).collect();
    let mut k = kids.into_iter();
    match &node.op {
        LogicalPlan::Scan { .. } | LogicalPlan::Values { .. } | LogicalPlan::CteRef { .. } => {
            node.op.clone()
        }
        LogicalPlan::Project { exprs, schema, .. } => LogicalPlan::Project {
            input: Box::new(k.next().expect("child")),
            exprs: exprs.clone(),
            schema: schema.clone(),
        },
        LogicalPlan::Filter { predicate, .. } => LogicalPlan::Filter {
            input: Box::new(k.next().expect("child")),
            predicate: predicate.clone(),
        },
        LogicalPlan::Join {
            kind, on, schema, ..
        } => LogicalPlan::Join {
            left: Box::new(k.next().expect("left")),
            right: Box::new(k.next().expect("right")),
            kind: *kind,
            on: on.clone(),
            schema: schema.clone(),
        },
        LogicalPlan::TableFunction {
            name,
            args,
            input,
            schema,
        } => LogicalPlan::TableFunction {
            name: name.clone(),
            args: args.clone(),
            input: input.as_ref().map(|_| Box::new(k.next().expect("input"))),
            schema: schema.clone(),
        },
        LogicalPlan::Aggregate {
            group_by,
            aggregates,
            schema,
            ..
        } => LogicalPlan::Aggregate {
            input: Box::new(k.next().expect("child")),
            group_by: group_by.clone(),
            aggregates: aggregates.clone(),
            schema: schema.clone(),
        },
        LogicalPlan::Sort { keys, .. } => LogicalPlan::Sort {
            input: Box::new(k.next().expect("child")),
            keys: keys.clone(),
        },
        LogicalPlan::Limit { offset, limit, .. } => LogicalPlan::Limit {
            input: Box::new(k.next().expect("child")),
            offset: *offset,
            limit: *limit,
        },
        LogicalPlan::Distinct { .. } => LogicalPlan::Distinct {
            input: Box::new(k.next().expect("child")),
        },
        LogicalPlan::Union { all, .. } => LogicalPlan::Union {
            inputs: k.collect(),
            all: *all,
        },
        LogicalPlan::With { ctes, .. } => {
            let mut rebuilt = vec![];
            for (name, _) in ctes {
                rebuilt.push((name.clone(), k.next().expect("cte")));
            }
            LogicalPlan::With {
                ctes: rebuilt,
                body: Box::new(k.next().expect("body")),
            }
        }
        LogicalPlan::Recursive {
            name,
            all,
            max_rounds,
            ..
        } => LogicalPlan::Recursive {
            name: name.clone(),
            base: Box::new(k.next().expect("base")),
            recursive: Box::new(k.next().expect("recursive")),
            all: *all,
            max_rounds: *max_rounds,
            body: Box::new(k.next().expect("body")),
        },
    }
}

/// Classify the query.
pub(crate) fn fragment(node: &CallNode) -> Fragment {
    fn walk(n: &CallNode, fo: &mut bool, rec: &mut bool) {
        match &n.op {
            LogicalPlan::Recursive { .. } => *rec = true,
            LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::Union { .. } => *fo = true,
            LogicalPlan::Join {
                kind: JoinKind::Left | JoinKind::Anti,
                ..
            } => *fo = true,
            LogicalPlan::Filter { predicate, .. } => {
                if has_negation(predicate) {
                    *fo = true;
                }
            }
            LogicalPlan::Project { exprs, .. } => {
                if exprs.iter().any(has_negation) {
                    *fo = true;
                }
            }
            _ => {}
        }
        for c in &n.children {
            walk(c, fo, rec);
        }
    }
    fn has_negation(e: &Expr) -> bool {
        match e {
            Expr::Exists { negated, subquery } => *negated || has_negation_plan(subquery),
            Expr::InSubquery {
                negated, subquery, ..
            } => *negated || has_negation_plan(subquery),
            Expr::Unary {
                op: callgebra_sql::UnaryOp::Not,
                ..
            } => true,
            Expr::Binary { left, right, .. } => has_negation(left) || has_negation(right),
            Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => has_negation(operand),
            Expr::Case { .. } | Expr::InList { negated: true, .. } => true,
            _ => false,
        }
    }
    fn has_negation_plan(p: &LogicalPlan) -> bool {
        match p {
            LogicalPlan::Filter { predicate, input } => {
                has_negation(predicate) || has_negation_plan(input)
            }
            LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::Recursive { .. } => true,
            other => other.children().into_iter().any(has_negation_plan),
        }
    }
    let (mut fo, mut rec) = (false, false);
    walk(node, &mut fo, &mut rec);
    if rec {
        Fragment::Recursive
    } else if fo {
        Fragment::FirstOrder
    } else {
        Fragment::Conjunctive
    }
}
