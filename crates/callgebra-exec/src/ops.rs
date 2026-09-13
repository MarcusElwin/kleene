//! Relational operators over materialised rows, and semi-naive recursion.

use crate::agg::Accumulator;
use crate::eval::eval_expr;
use crate::{builtins, ExecContext, ExecError, RECURSION_HARD_CAP};
use async_recursion::async_recursion;
use callgebra_core::{Row, Value};
use callgebra_sql::{Expr, JoinKind, LogicalPlan, SortKey};
use futures::StreamExt;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Evaluation environment: bound CTEs and the stack of enclosing rows.
#[derive(Clone, Default)]
pub(crate) struct Env {
    /// CTE name (lower case) to materialised rows.
    pub ctes: HashMap<String, Arc<Vec<Row>>>,
    /// Rows of enclosing queries, outermost first.
    pub outer: Vec<Row>,
}

impl Env {
    /// A copy with `row` pushed as the innermost enclosing row.
    pub fn push_outer(&self, row: &[Value]) -> Env {
        let mut e = self.clone();
        e.outer.push(row.to_vec());
        e
    }

    /// A copy with a CTE bound.
    pub fn bind(&self, name: &str, rows: Arc<Vec<Row>>) -> Env {
        let mut e = self.clone();
        e.ctes.insert(name.to_ascii_lowercase(), rows);
        e
    }
}

async fn eval_exprs(
    exprs: &[Expr],
    row: &[Value],
    env: &Env,
    ctx: &ExecContext,
) -> Result<Row, ExecError> {
    let mut out = Vec::with_capacity(exprs.len());
    for e in exprs {
        out.push(eval_expr(e, row, env, ctx).await?);
    }
    Ok(out)
}

async fn is_true(
    pred: &Expr,
    row: &[Value],
    env: &Env,
    ctx: &ExecContext,
) -> Result<bool, ExecError> {
    Ok(eval_expr(pred, row, env, ctx).await? == Value::Bool(true))
}

fn dedupe(rows: Vec<Row>) -> Vec<Row> {
    let mut seen: HashSet<Row> = HashSet::with_capacity(rows.len());
    rows.into_iter()
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

fn concat(a: &[Value], b: &[Value]) -> Row {
    let mut r = Vec::with_capacity(a.len() + b.len());
    r.extend_from_slice(a);
    r.extend_from_slice(b);
    r
}

/// Evaluate a plan to rows.
#[async_recursion]
pub(crate) async fn eval_plan(
    plan: &LogicalPlan,
    env: &Env,
    ctx: &ExecContext,
) -> Result<Vec<Row>, ExecError> {
    match plan {
        LogicalPlan::Scan { table, .. } => {
            let mut stream = ctx.sink.scan(table).await?;
            let mut rows = vec![];
            while let Some(b) = stream.next().await {
                rows.extend(b?.rows);
            }
            Ok(rows)
        }
        LogicalPlan::Values { rows, .. } => {
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                out.push(eval_exprs(r, &[], env, ctx).await?);
            }
            Ok(out)
        }
        LogicalPlan::CteRef { name, .. } => env
            .ctes
            .get(&name.to_ascii_lowercase())
            .map(|r| r.as_ref().clone())
            .ok_or_else(|| ExecError::Eval(format!("unbound CTE {name}"))),
        LogicalPlan::Project { input, exprs, .. } => {
            let rows = eval_plan(input, env, ctx).await?;
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                out.push(eval_exprs(exprs, r, env, ctx).await?);
            }
            Ok(out)
        }
        LogicalPlan::Filter { input, predicate } => {
            let rows = eval_plan(input, env, ctx).await?;
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                if is_true(predicate, &r, env, ctx).await? {
                    out.push(r);
                }
            }
            Ok(out)
        }
        LogicalPlan::Join {
            left,
            right,
            kind,
            on,
            ..
        } => {
            let l = eval_plan(left, env, ctx).await?;
            let r = eval_plan(right, env, ctx).await?;
            let right_width = right.schema().len();
            let mut out = vec![];
            for lrow in &l {
                let mut matched = false;
                for rrow in &r {
                    let joined = concat(lrow, rrow);
                    let ok = match on {
                        None => true,
                        Some(p) => is_true(p, &joined, env, ctx).await?,
                    };
                    if !ok {
                        continue;
                    }
                    matched = true;
                    match kind {
                        JoinKind::Inner | JoinKind::Left | JoinKind::Cross => out.push(joined),
                        JoinKind::Semi => {
                            out.push(lrow.clone());
                            break;
                        }
                        JoinKind::Anti => break,
                    }
                }
                match kind {
                    JoinKind::Left if !matched => {
                        let mut padded = lrow.clone();
                        padded.extend(std::iter::repeat_n(Value::Null, right_width));
                        out.push(padded);
                    }
                    JoinKind::Anti if !matched => out.push(lrow.clone()),
                    _ => {}
                }
            }
            Ok(out)
        }
        LogicalPlan::TableFunction {
            name, args, input, ..
        } => {
            let call = |vals: Vec<Value>| async move {
                if name == "generate_series" {
                    builtins::generate_series(&vals)
                } else {
                    Ok(ctx.sink.table_call(name, &vals).await?.rows)
                }
            };
            match input {
                None => {
                    let vals = eval_exprs(args, &[], env, ctx).await?;
                    call(vals).await
                }
                Some(inp) => {
                    let rows = eval_plan(inp, env, ctx).await?;
                    let mut out = vec![];
                    for r in &rows {
                        let vals = eval_exprs(args, r, env, ctx).await?;
                        for frow in call(vals).await? {
                            out.push(concat(r, &frow));
                        }
                    }
                    Ok(out)
                }
            }
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            ..
        } => {
            let rows = eval_plan(input, env, ctx).await?;
            let specs: Vec<(callgebra_sql::AggregateFn, bool, &Vec<Expr>)> = aggregates
                .iter()
                .map(|a| match a {
                    Expr::Aggregate {
                        func,
                        args,
                        distinct,
                    } => Ok((*func, *distinct, args)),
                    other => Err(ExecError::Eval(format!(
                        "non-aggregate in aggregate list: {other:?}"
                    ))),
                })
                .collect::<Result<_, _>>()?;
            let mut order: Vec<Row> = vec![];
            let mut groups: HashMap<Row, Vec<Accumulator>> = HashMap::new();
            for r in &rows {
                let key = eval_exprs(group_by, r, env, ctx).await?;
                let accs = match groups.get_mut(&key) {
                    Some(a) => a,
                    None => {
                        order.push(key.clone());
                        groups.entry(key).or_insert_with(|| {
                            specs
                                .iter()
                                .map(|(f, d, _)| Accumulator::new(*f, *d))
                                .collect()
                        })
                    }
                };
                for (acc, (_, _, args)) in accs.iter_mut().zip(&specs) {
                    let vals = eval_exprs(args, r, env, ctx).await?;
                    acc.push(&vals)?;
                }
            }
            if groups.is_empty() && group_by.is_empty() {
                let accs: Vec<Accumulator> = specs
                    .iter()
                    .map(|(f, d, _)| Accumulator::new(*f, *d))
                    .collect();
                return Ok(vec![accs.into_iter().map(Accumulator::finish).collect()]);
            }
            let mut out = Vec::with_capacity(order.len());
            for key in order {
                let accs = groups.remove(&key).expect("group present");
                let mut row = key;
                row.extend(accs.into_iter().map(Accumulator::finish));
                out.push(row);
            }
            Ok(out)
        }
        LogicalPlan::Sort { input, keys } => {
            let rows = eval_plan(input, env, ctx).await?;
            let mut keyed: Vec<(Vec<Value>, Row)> = Vec::with_capacity(rows.len());
            for r in rows {
                let k = eval_exprs(
                    &keys.iter().map(|k| k.expr.clone()).collect::<Vec<_>>(),
                    &r,
                    env,
                    ctx,
                )
                .await?;
                keyed.push((k, r));
            }
            let error: Cell<Option<ExecError>> = Cell::new(None);
            keyed.sort_by(|(a, _), (b, _)| compare_keys(a, b, keys, &error));
            if let Some(e) = error.take() {
                return Err(e);
            }
            Ok(keyed.into_iter().map(|(_, r)| r).collect())
        }
        LogicalPlan::Limit {
            input,
            offset,
            limit,
        } => {
            let rows = eval_plan(input, env, ctx).await?;
            Ok(rows
                .into_iter()
                .skip(*offset)
                .take(limit.unwrap_or(usize::MAX))
                .collect())
        }
        LogicalPlan::Distinct { input } => Ok(dedupe(eval_plan(input, env, ctx).await?)),
        LogicalPlan::Union { inputs, all } => {
            let mut out = vec![];
            for i in inputs {
                out.extend(eval_plan(i, env, ctx).await?);
            }
            Ok(if *all { out } else { dedupe(out) })
        }
        LogicalPlan::With { ctes, body } => {
            let mut e = env.clone();
            for (name, p) in ctes {
                let rows = eval_plan(p, &e, ctx).await?;
                e = e.bind(name, Arc::new(rows));
            }
            eval_plan(body, &e, ctx).await
        }
        LogicalPlan::Recursive {
            name,
            base,
            recursive,
            all,
            max_rounds,
            body,
        } => {
            let cap = max_rounds.or(ctx.max_recursion_rounds);
            let mut total = eval_plan(base, env, ctx).await?;
            if !*all {
                total = dedupe(total);
            }
            let mut seen: HashSet<Row> = if *all {
                HashSet::new()
            } else {
                total.iter().cloned().collect()
            };
            let mut delta = total.clone();
            let mut round: usize = 0;
            tracing::debug!(cte = %name, round, delta_rows = delta.len(), total_rows = total.len(), "recursion round");
            while !delta.is_empty() {
                if cap.is_some_and(|c| round >= c) {
                    break;
                }
                round += 1;
                if round > RECURSION_HARD_CAP {
                    return Err(ExecError::Eval(format!(
                        "recursive CTE {name} exceeded {RECURSION_HARD_CAP} rounds"
                    )));
                }
                let e = env.bind(name, Arc::new(delta));
                let mut new = eval_plan(recursive, &e, ctx).await?;
                if !*all {
                    let mut kept = Vec::with_capacity(new.len());
                    for r in new {
                        if seen.insert(r.clone()) {
                            kept.push(r);
                        }
                    }
                    new = kept;
                }
                tracing::debug!(cte = %name, round, delta_rows = new.len(), total_rows = total.len() + new.len(), "recursion round");
                total.extend(new.iter().cloned());
                delta = new;
            }
            let e = env.bind(name, Arc::new(total));
            eval_plan(body, &e, ctx).await
        }
    }
}

fn compare_keys(
    a: &[Value],
    b: &[Value],
    keys: &[SortKey],
    error: &Cell<Option<ExecError>>,
) -> Ordering {
    for (i, k) in keys.iter().enumerate() {
        let (x, y) = (&a[i], &b[i]);
        let ord = match (x.is_null(), y.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => {
                if k.nulls_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (false, true) => {
                if k.nulls_first {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (false, false) => match x.sql_cmp(y) {
                Some(o) => {
                    if k.asc {
                        o
                    } else {
                        o.reverse()
                    }
                }
                None => {
                    if error.take().is_none() {
                        error.set(Some(ExecError::Eval(format!(
                            "cannot order {} against {}",
                            x.data_type(),
                            y.data_type()
                        ))));
                    }
                    Ordering::Equal
                }
            },
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}
