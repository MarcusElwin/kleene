//! Annotate a logical plan with call kinds, volatility and estimates.

use crate::{CallNode, Estimate, Fragment};
use kleene_core::{CallKind, Catalog, FunctionDef, Volatility};
use kleene_sql::{Expr, JoinKind, LogicalPlan};
use std::collections::HashMap;

/// Parameters of the cost model: the store's row counts, the defaults, and
/// what the harness has learned (selectivities and branching factors from
/// the actuals of earlier statements, kept in the store's `estimates` table
/// and sampled again within the session).
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
    /// Observed pass rate of boolean call predicates, by function name
    /// (lower case): the sampled selectivity. Missing names fall back to
    /// `filter_selectivity`.
    pub call_selectivity: HashMap<String, f64>,
    /// Token and dollar multiplier per model alias, relative to `worker`.
    /// Missing aliases cost 1.0; `proxy` defaults to 0.1.
    pub alias_factor: HashMap<String, f64>,
    /// Observed rows per call of table functions, by function name (lower
    /// case): the learned branching factor. Missing names fall back to
    /// `branching`.
    pub call_branching: HashMap<String, f64>,
    /// How many observations stand behind a learned entry of
    /// `call_selectivity` or `call_branching`, by function name; `EXPLAIN`
    /// lists them so the model knows which estimates are measured.
    pub learned: HashMap<String, u64>,
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
            call_selectivity: HashMap::new(),
            alias_factor: HashMap::from([("proxy".to_string(), 0.1)]),
            call_branching: HashMap::new(),
            learned: HashMap::new(),
        }
    }
}

impl CostModel {
    /// Cost of `n` calls on the worker tier.
    pub fn calls(&self, n: f64) -> Estimate {
        self.calls_factor(n, 1.0)
    }

    fn calls_factor(&self, n: f64, factor: f64) -> Estimate {
        let tokens = n * (self.tokens_in_per_call + self.tokens_out_per_call) * factor;
        let dollars = n
            * (self.tokens_in_per_call * self.usd_per_input_token
                + self.tokens_out_per_call * self.usd_per_output_token)
            * factor;
        Estimate {
            rows: 0.0,
            calls: n,
            tokens,
            dollars,
            depth: 0,
        }
    }

    /// Cost multiplier of a model alias.
    pub fn factor(&self, alias: &str) -> f64 {
        self.alias_factor
            .get(&alias.to_ascii_lowercase())
            .copied()
            .unwrap_or(1.0)
    }

    /// Cost of `n` invocations of each call in `kinds`.
    pub fn calls_for(&self, kinds: &[CallKind], n: f64) -> Estimate {
        let mut est = Estimate::default();
        for k in kinds {
            let factor = match k {
                CallKind::Pure => continue,
                CallKind::Tool { .. } => 0.0,
                CallKind::LlmScalar { alias, .. } | CallKind::LlmTable { alias } => {
                    self.factor(&alias.0)
                }
                CallKind::Recursive { .. } => 1.0,
            };
            // A batchable prompt answers `batch` tuples per call.
            let n = match k {
                CallKind::LlmScalar { batch: Some(b), .. } if *b > 1 => (n / *b as f64).ceil(),
                _ => n,
            };
            est = est.plus(self.calls_factor(n, factor));
        }
        est
    }

    /// Selectivity of one boolean call predicate by name.
    pub fn selectivity_of(&self, function: &str) -> Option<f64> {
        self.call_selectivity
            .get(&function.to_ascii_lowercase())
            .copied()
    }

    /// Rows one call of a table function produces: the learned factor for
    /// its name, else the default `branching`.
    pub fn branching_of(&self, function: &str) -> f64 {
        self.call_branching
            .get(&function.to_ascii_lowercase())
            .copied()
            .unwrap_or(self.branching)
    }

    /// One line per learned estimate the plan relies on, for `EXPLAIN`:
    /// `name: selectivity 0.75 from 12 rows`.
    pub fn learned_notes(&self, call_names: &[String]) -> Vec<String> {
        let mut seen: Vec<&String> = vec![];
        let mut out = vec![];
        for name in call_names {
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
            let Some(n) = self.learned.get(name) else {
                continue;
            };
            if let Some(sel) = self.call_selectivity.get(name) {
                out.push(format!("{name}: selectivity {sel:.2} from {n} rows"));
            } else if let Some(b) = self.call_branching.get(name) {
                out.push(format!("{name}: {b:.1} rows per call from {n} calls"));
            }
        }
        out
    }
}

/// Names of the call functions a plan evaluates (lower case, with repeats,
/// in evaluation order).
pub fn plan_call_names(plan: &LogicalPlan, catalog: &Catalog, out: &mut Vec<String>) {
    for child in plan.children() {
        plan_call_names(child, catalog, out);
    }
    match plan {
        LogicalPlan::Values { rows, .. } => {
            for e in rows.iter().flatten() {
                call_names(e, catalog, out);
            }
        }
        LogicalPlan::Project { exprs, .. } => {
            for e in exprs {
                call_names(e, catalog, out);
            }
        }
        LogicalPlan::Filter { predicate, .. } => call_names(predicate, catalog, out),
        LogicalPlan::Join { on: Some(on), .. } => call_names(on, catalog, out),
        LogicalPlan::TableFunction { name, args, .. } => {
            for a in args {
                call_names(a, catalog, out);
            }
            if catalog
                .function(name)
                .is_some_and(|d| d.call_kind != CallKind::Pure)
            {
                out.push(name.to_ascii_lowercase());
            }
        }
        LogicalPlan::Aggregate {
            group_by,
            aggregates,
            ..
        } => {
            for e in group_by.iter().chain(aggregates) {
                call_names(e, catalog, out);
            }
        }
        LogicalPlan::Sort { keys, .. } => {
            for k in keys {
                call_names(&k.expr, catalog, out);
            }
        }
        _ => {}
    }
}

/// Top-level conjuncts of a predicate.
pub(crate) fn conjuncts(e: &Expr) -> Vec<Expr> {
    match e {
        Expr::Binary {
            op: kleene_sql::BinaryOp::And,
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

/// Names of call functions in an expression (lower case, with repeats).
pub(crate) fn call_names(e: &Expr, catalog: &Catalog, out: &mut Vec<String>) {
    match e {
        Expr::Function { name, args, .. } => {
            for a in args {
                call_names(a, catalog, out);
            }
            if catalog
                .function(name)
                .is_some_and(|d| d.call_kind != CallKind::Pure)
            {
                out.push(name.to_ascii_lowercase());
            }
        }
        Expr::Binary { left, right, .. } => {
            call_names(left, catalog, out);
            call_names(right, catalog, out);
        }
        Expr::Unary { operand, .. } | Expr::Cast { operand, .. } => {
            call_names(operand, catalog, out)
        }
        Expr::Aggregate { args, .. } => {
            for a in args {
                call_names(a, catalog, out);
            }
        }
        Expr::Case {
            branches,
            otherwise,
        } => {
            for (c, r) in branches {
                call_names(c, catalog, out);
                call_names(r, catalog, out);
            }
            if let Some(o) = otherwise {
                call_names(o, catalog, out);
            }
        }
        Expr::InList { operand, list, .. } => {
            call_names(operand, catalog, out);
            for l in list {
                call_names(l, catalog, out);
            }
        }
        Expr::InSubquery { operand, .. } => call_names(operand, catalog, out),
        Expr::Exists { .. }
        | Expr::ScalarSubquery { .. }
        | Expr::Column { .. }
        | Expr::Literal(_)
        | Expr::OuterColumn { .. } => {}
    }
}

/// Whether a pure conjunct is an equality between two columns (a join key).
pub(crate) fn is_equijoin(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Binary {
            op: kleene_sql::BinaryOp::Eq,
            left,
            right
        } if matches!(**left, Expr::Column { .. }) && matches!(**right, Expr::Column { .. })
    )
}

/// The fraction of rows that reach the oracle in a cascade conjunct of the
/// shape `p >= high OR (p >= low AND oracle(...))`: `high - low`.
fn cascade_band(e: &Expr) -> Option<f64> {
    use kleene_sql::BinaryOp;
    let Expr::Binary {
        op: BinaryOp::Or,
        left,
        right,
    } = e
    else {
        return None;
    };
    let hi = threshold(left)?;
    let Expr::Binary {
        op: BinaryOp::And,
        left: band,
        ..
    } = &**right
    else {
        return None;
    };
    let lo = threshold(band)?;
    Some((hi - lo).clamp(0.0, 1.0))
}

fn threshold(e: &Expr) -> Option<f64> {
    let Expr::Binary {
        op: kleene_sql::BinaryOp::GtEq,
        right,
        ..
    } = e
    else {
        return None;
    };
    match &**right {
        Expr::Literal(l) => match &l.0 {
            kleene_core::Value::Float(f) => Some(*f),
            kleene_core::Value::Int(i) => Some(*i as f64),
            _ => None,
        },
        _ => None,
    }
}

/// Cost of evaluating an ordered conjunction over `rows` inputs the way the
/// executor does (left to right, short-circuiting): pure conjuncts narrow
/// the survivors, call conjuncts pay one call per survivor. Returns the
/// estimate (rows = survivors) and the calls found.
pub(crate) fn predicate_cost(
    pred: &Expr,
    catalog: &Catalog,
    cost: &CostModel,
    rows: f64,
    join_keys: bool,
) -> (Estimate, Vec<CallKind>) {
    let mut survivors = rows;
    let mut est = Estimate::default();
    let mut all_calls = vec![];
    for c in conjuncts(pred) {
        let mut kinds = vec![];
        calls_in_expr(&c, catalog, &mut kinds);
        let (sub, _) = subquery_cost(std::slice::from_ref(&c), catalog, cost, survivors);
        est = est.plus(sub);
        if kinds.is_empty() {
            survivors *= if join_keys && is_equijoin(&c) {
                cost.join_selectivity
            } else {
                cost.filter_selectivity
            };
            continue;
        }
        let band = cascade_band(&c).unwrap_or(1.0);
        let n = survivors * band * cost.distinct_fraction;
        est = est.plus(cost.calls_for(&kinds, n));
        let mut names = vec![];
        call_names(&c, catalog, &mut names);
        let mut sel = 1.0;
        let mut known = false;
        for name in &names {
            if let Some(s) = cost.selectivity_of(name) {
                sel *= s;
                known = true;
            }
        }
        if !known {
            sel = cost.filter_selectivity;
        }
        survivors *= sel;
        all_calls.extend(kinds);
    }
    est.rows = survivors.max(0.0);
    (est, all_calls)
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
    annotate_in(plan, catalog, cost, &HashMap::new())
}

/// The recursive term's `LIMIT k` (possibly under a projection), which makes
/// the fixpoint a beam search with frontier `k`.
pub fn beam_of(recursive: &LogicalPlan) -> Option<usize> {
    match recursive {
        LogicalPlan::Limit { limit: Some(k), .. } => Some(*k),
        LogicalPlan::Project { input, .. } => beam_of(input),
        _ => None,
    }
}

/// Rounds a recursion will run: `MAXRECURSION` if given, else the model's guess.
pub fn rounds_of(max_rounds: Option<usize>, cost: &CostModel) -> f64 {
    max_rounds
        .map(|r| r as f64)
        .unwrap_or(cost.recursion_rounds)
        .max(1.0)
}

fn annotate_in(
    plan: &LogicalPlan,
    catalog: &Catalog,
    cost: &CostModel,
    cte_rows: &HashMap<String, f64>,
) -> CallNode {
    let children: Vec<CallNode> = match plan {
        LogicalPlan::With { ctes, body } => {
            let mut scope = cte_rows.clone();
            let mut kids = vec![];
            for (name, p) in ctes {
                let node = annotate_in(p, catalog, cost, &scope);
                scope.insert(name.to_ascii_lowercase(), node.estimate.rows);
                kids.push(node);
            }
            kids.push(annotate_in(body, catalog, cost, &scope));
            kids
        }
        LogicalPlan::Recursive {
            name,
            base,
            recursive,
            body,
            ..
        } => {
            let base_node = annotate_in(base, catalog, cost, cte_rows);
            let mut scope = cte_rows.clone();
            scope.insert(name.to_ascii_lowercase(), base_node.estimate.rows);
            let rec_node = annotate_in(recursive, catalog, cost, &scope);
            let total = fixpoint_rows(plan, &base_node, &rec_node, catalog, cost, cte_rows).0;
            scope.insert(name.to_ascii_lowercase(), total);
            vec![
                base_node,
                rec_node,
                annotate_in(body, catalog, cost, &scope),
            ]
        }
        other => other
            .children()
            .into_iter()
            .map(|c| annotate_in(c, catalog, cost, cte_rows))
            .collect(),
    };
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
        LogicalPlan::CteRef { name, .. } => (
            vec![],
            Volatility::Stable,
            Estimate {
                rows: cte_rows
                    .get(&name.to_ascii_lowercase())
                    .copied()
                    .unwrap_or(cost.default_rows),
                ..Estimate::default()
            },
        ),
        LogicalPlan::Project { exprs, .. } => {
            let calls = expr_list_calls(exprs, catalog);
            let mut est = cost.calls_for(&calls, in_rows * cost.distinct_fraction);
            est.rows = in_rows;
            let (sub, _) = subquery_cost(exprs, catalog, cost, in_rows);
            (calls, expr_list_volatility(exprs, catalog), est.plus(sub))
        }
        LogicalPlan::Filter { predicate, .. } => {
            let (est, calls) = predicate_cost(predicate, catalog, cost, in_rows, false);
            (calls, volatility_of_expr(predicate, catalog), est)
        }
        LogicalPlan::Join { kind, on, .. } => {
            let l = children.first().map(|c| c.estimate.rows).unwrap_or(1.0);
            let r = children.get(1).map(|c| c.estimate.rows).unwrap_or(1.0);
            let pairs = l * r;
            let (mut est, calls) = match on {
                Some(p) => predicate_cost(p, catalog, cost, pairs, true),
                None => (
                    Estimate {
                        rows: pairs,
                        ..Estimate::default()
                    },
                    vec![],
                ),
            };
            let matched = est.rows;
            // A semi/anti join keeps left rows that have (no) match: the
            // fraction of left rows with at least one match.
            let left_hit = (matched / r.max(1.0))
                .min(l)
                .max(l * cost.filter_selectivity);
            est.rows = match kind {
                JoinKind::Cross => pairs,
                JoinKind::Inner => matched.max(1.0),
                JoinKind::Left => matched.max(l),
                JoinKind::Semi => left_hit,
                JoinKind::Anti => (l - left_hit).max(0.0),
            };
            let exprs: Vec<Expr> = on.iter().cloned().collect();
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
            let mut est = cost.calls_for(&calls, per_input);
            match &kind {
                CallKind::Pure => {}
                k => {
                    calls.push(k.clone());
                    if matches!(k, CallKind::LlmTable { .. } | CallKind::Recursive { .. }) {
                        est = est.plus(cost.calls_for(std::slice::from_ref(k), per_input));
                    }
                }
            }
            est.rows = per_input * cost.branching_of(name);
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
            let mut est = cost.calls_for(&calls, in_rows);
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
            let mut est = cost.calls_for(&calls, in_rows);
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
            // children: base, recursive, body. The recursive term runs once
            // per round over that round's frontier; the child node shows the
            // first round, so the remaining rounds are added here.
            let (_, extra) =
                fixpoint_rows(plan, &children[0], &children[1], catalog, cost, cte_rows);
            let body_rows = children.get(2).map(|c| c.estimate.rows).unwrap_or(in_rows);
            (
                vec![],
                Volatility::Immutable,
                Estimate {
                    rows: body_rows,
                    calls: extra.calls,
                    tokens: extra.tokens,
                    dollars: extra.dollars,
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

/// Simulate the rounds of a fixpoint: the total rows produced and the cost
/// of every round after the first (the first is the recursive child's own
/// estimate). Round `i + 1` runs the recursive term over round `i`'s
/// frontier, so a beam `LIMIT k` caps every frontier at `k`; the loop stops
/// when the frontier empties or the round cap hits.
fn fixpoint_rows(
    plan: &LogicalPlan,
    base: &CallNode,
    first_round: &CallNode,
    catalog: &Catalog,
    cost: &CostModel,
    cte_rows: &HashMap<String, f64>,
) -> (f64, Estimate) {
    let LogicalPlan::Recursive {
        name,
        recursive,
        max_rounds,
        ..
    } = plan
    else {
        return (base.estimate.rows, Estimate::default());
    };
    let rounds = rounds_of(*max_rounds, cost) as usize;
    let mut total = base.estimate.rows;
    let mut frontier = first_round.estimate.rows;
    total += frontier;
    let mut extra = Estimate::default();
    for _ in 1..rounds {
        if frontier < 0.5 {
            break;
        }
        let mut scope = cte_rows.clone();
        scope.insert(name.to_ascii_lowercase(), frontier);
        let round = annotate_in(recursive, catalog, cost, &scope);
        let t = round.total();
        extra = extra.plus(Estimate {
            rows: 0.0,
            calls: t.calls,
            tokens: t.tokens,
            dollars: t.dollars,
            depth: t.depth,
        });
        frontier = round.estimate.rows;
        total += frontier;
    }
    (total, extra)
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
            LogicalPlan::Project { exprs, .. } if exprs.iter().any(has_negation) => {
                *fo = true;
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
                op: kleene_sql::UnaryOp::Not,
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
