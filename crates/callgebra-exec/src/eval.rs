//! Expression evaluation with SQL three-valued logic.

use crate::ops::{eval_plan, Env};
use crate::{builtins, like, ExecContext, ExecError};
use async_recursion::async_recursion;
use callgebra_core::Value;
use callgebra_sql::{BinaryOp, Expr, UnaryOp};
use std::cmp::Ordering;

fn bool3(v: &Value, what: &str) -> Result<Option<bool>, ExecError> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        other => Err(ExecError::Eval(format!(
            "{what} expects a boolean, got {}",
            other.data_type()
        ))),
    }
}

fn from_bool3(b: Option<bool>) -> Value {
    b.map(Value::Bool).unwrap_or(Value::Null)
}

fn arith(op: BinaryOp, l: &Value, r: &Value) -> Result<Value, ExecError> {
    if l.is_null() || r.is_null() {
        return Ok(Value::Null);
    }
    let overflow = || ExecError::Eval(format!("integer overflow in {op:?}"));
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(match op {
            BinaryOp::Plus => Value::Int(a.checked_add(*b).ok_or_else(overflow)?),
            BinaryOp::Minus => Value::Int(a.checked_sub(*b).ok_or_else(overflow)?),
            BinaryOp::Multiply => Value::Int(a.checked_mul(*b).ok_or_else(overflow)?),
            BinaryOp::Divide => {
                if *b == 0 {
                    Value::Null
                } else {
                    Value::Float(*a as f64 / *b as f64)
                }
            }
            BinaryOp::Modulo => {
                if *b == 0 {
                    return Err(ExecError::Eval("modulo by zero".into()));
                }
                Value::Int(a.wrapping_rem(*b))
            }
            _ => unreachable!("not arithmetic"),
        }),
        _ => {
            let (a, b) = match (l.as_f64(), r.as_f64()) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    return Err(ExecError::Eval(format!(
                        "arithmetic on {} and {}",
                        l.data_type(),
                        r.data_type()
                    )))
                }
            };
            Ok(match op {
                BinaryOp::Plus => Value::Float(a + b),
                BinaryOp::Minus => Value::Float(a - b),
                BinaryOp::Multiply => Value::Float(a * b),
                BinaryOp::Divide => {
                    if b == 0.0 {
                        Value::Null
                    } else {
                        Value::Float(a / b)
                    }
                }
                BinaryOp::Modulo => {
                    if b == 0.0 {
                        return Err(ExecError::Eval("modulo by zero".into()));
                    }
                    Value::Float(a % b)
                }
                _ => unreachable!("not arithmetic"),
            })
        }
    }
}

fn compare(op: BinaryOp, l: &Value, r: &Value) -> Result<Value, ExecError> {
    if l.is_null() || r.is_null() {
        return Ok(Value::Null);
    }
    match op {
        BinaryOp::Eq => Ok(from_bool3(l.sql_eq(r))),
        BinaryOp::NotEq => Ok(from_bool3(l.sql_eq(r).map(|b| !b))),
        _ => {
            let ord = l.sql_cmp(r).ok_or_else(|| {
                ExecError::Eval(format!(
                    "cannot compare {} with {}",
                    l.data_type(),
                    r.data_type()
                ))
            })?;
            Ok(Value::Bool(match op {
                BinaryOp::Lt => ord == Ordering::Less,
                BinaryOp::LtEq => ord != Ordering::Greater,
                BinaryOp::Gt => ord == Ordering::Greater,
                BinaryOp::GtEq => ord != Ordering::Less,
                _ => unreachable!("not a comparison"),
            }))
        }
    }
}

/// `x IN (values)` with SQL NULL semantics over already-evaluated candidates.
pub(crate) fn in_values(x: &Value, candidates: &[Value], negated: bool) -> Value {
    if x.is_null() {
        return Value::Null;
    }
    let mut saw_null = false;
    for c in candidates {
        match x.sql_eq(c) {
            Some(true) => return Value::Bool(!negated),
            Some(false) => {}
            None => saw_null = true,
        }
    }
    if saw_null {
        Value::Null
    } else {
        Value::Bool(negated)
    }
}

/// Evaluate `e` against `row`, with `env.outer` holding enclosing rows for
/// correlated subqueries.
#[async_recursion]
pub(crate) async fn eval_expr(
    e: &Expr,
    row: &[Value],
    env: &Env,
    ctx: &ExecContext,
) -> Result<Value, ExecError> {
    Ok(match e {
        Expr::Column { index, name } => row
            .get(*index)
            .cloned()
            .ok_or_else(|| ExecError::Eval(format!("column {name} (#{index}) out of range")))?,
        Expr::OuterColumn { depth, index, name } => {
            let len = env.outer.len();
            if *depth == 0 || *depth > len {
                return Err(ExecError::Eval(format!(
                    "outer column {name} at depth {depth} has no enclosing row"
                )));
            }
            env.outer[len - depth]
                .get(*index)
                .cloned()
                .ok_or_else(|| ExecError::Eval(format!("outer column {name} out of range")))?
        }
        Expr::Literal(l) => l.0.clone(),
        Expr::Binary { op, left, right } => match op {
            BinaryOp::And => {
                let l = bool3(&eval_expr(left, row, env, ctx).await?, "AND")?;
                if l == Some(false) {
                    return Ok(Value::Bool(false));
                }
                let r = bool3(&eval_expr(right, row, env, ctx).await?, "AND")?;
                from_bool3(match (l, r) {
                    (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                })
            }
            BinaryOp::Or => {
                let l = bool3(&eval_expr(left, row, env, ctx).await?, "OR")?;
                if l == Some(true) {
                    return Ok(Value::Bool(true));
                }
                let r = bool3(&eval_expr(right, row, env, ctx).await?, "OR")?;
                from_bool3(match (l, r) {
                    (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                })
            }
            _ => {
                let l = eval_expr(left, row, env, ctx).await?;
                let r = eval_expr(right, row, env, ctx).await?;
                match op {
                    BinaryOp::Plus
                    | BinaryOp::Minus
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Modulo => arith(*op, &l, &r)?,
                    BinaryOp::Eq
                    | BinaryOp::NotEq
                    | BinaryOp::Lt
                    | BinaryOp::LtEq
                    | BinaryOp::Gt
                    | BinaryOp::GtEq => compare(*op, &l, &r)?,
                    BinaryOp::Concat => {
                        if l.is_null() || r.is_null() {
                            Value::Null
                        } else {
                            Value::Text(format!("{}{}", l.render(), r.render()))
                        }
                    }
                    BinaryOp::Like | BinaryOp::ILike => match (&l, &r) {
                        (Value::Null, _) | (_, Value::Null) => Value::Null,
                        (Value::Text(t), Value::Text(p)) => Value::Bool(if *op == BinaryOp::Like {
                            like::like(t, p)
                        } else {
                            like::ilike(t, p)
                        }),
                        _ => {
                            return Err(ExecError::Eval(format!(
                                "LIKE needs text operands, got {} and {}",
                                l.data_type(),
                                r.data_type()
                            )))
                        }
                    },
                    BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
                }
            }
        },
        Expr::Unary { op, operand } => {
            let v = eval_expr(operand, row, env, ctx).await?;
            match op {
                UnaryOp::Not => from_bool3(bool3(&v, "NOT")?.map(|b| !b)),
                UnaryOp::Neg => {
                    match v {
                        Value::Null => Value::Null,
                        Value::Int(i) => Value::Int(i.checked_neg().ok_or_else(|| {
                            ExecError::Eval("integer overflow in negation".into())
                        })?),
                        Value::Float(f) => Value::Float(-f),
                        other => {
                            return Err(ExecError::Eval(format!(
                                "cannot negate {}",
                                other.data_type()
                            )))
                        }
                    }
                }
                UnaryOp::IsNull => Value::Bool(v.is_null()),
                UnaryOp::IsNotNull => Value::Bool(!v.is_null()),
            }
        }
        Expr::Function { name, args, .. } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_expr(a, row, env, ctx).await?);
            }
            if builtins::is_builtin(name) {
                builtins::call(name, &vals)?
            } else {
                ctx.sink.scalar_call(name, &vals).await?
            }
        }
        Expr::Aggregate { .. } => {
            return Err(ExecError::Eval(
                "aggregate function outside an aggregation".into(),
            ))
        }
        Expr::Cast { operand, to } => eval_expr(operand, row, env, ctx)
            .await?
            .cast(*to)
            .map_err(|e| ExecError::Eval(e.to_string()))?,
        Expr::Case {
            branches,
            otherwise,
        } => {
            for (cond, result) in branches {
                if eval_expr(cond, row, env, ctx).await? == Value::Bool(true) {
                    return eval_expr(result, row, env, ctx).await;
                }
            }
            match otherwise {
                Some(o) => eval_expr(o, row, env, ctx).await?,
                None => Value::Null,
            }
        }
        Expr::InList {
            operand,
            list,
            negated,
        } => {
            let x = eval_expr(operand, row, env, ctx).await?;
            let mut vals = Vec::with_capacity(list.len());
            for a in list {
                vals.push(eval_expr(a, row, env, ctx).await?);
            }
            in_values(&x, &vals, *negated)
        }
        Expr::Exists { subquery, negated } => {
            let inner = env.push_outer(row);
            let rows = eval_plan(subquery, &inner, ctx).await?;
            Value::Bool(rows.is_empty() == *negated)
        }
        Expr::InSubquery {
            operand,
            subquery,
            negated,
        } => {
            let x = eval_expr(operand, row, env, ctx).await?;
            let inner = env.push_outer(row);
            let rows = eval_plan(subquery, &inner, ctx).await?;
            let vals: Vec<Value> = rows.into_iter().map(|mut r| r.swap_remove(0)).collect();
            in_values(&x, &vals, *negated)
        }
        Expr::ScalarSubquery { subquery } => {
            let inner = env.push_outer(row);
            let mut rows = eval_plan(subquery, &inner, ctx).await?;
            match rows.len() {
                0 => Value::Null,
                1 => rows.swap_remove(0).swap_remove(0),
                n => {
                    return Err(ExecError::Eval(format!(
                        "scalar subquery returned {n} rows"
                    )))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_valued_in_list() {
        let one = Value::Int(1);
        assert_eq!(
            in_values(&one, &[Value::Int(1), Value::Null], false),
            Value::Bool(true)
        );
        assert_eq!(
            in_values(&one, &[Value::Int(2), Value::Null], false),
            Value::Null
        );
        assert_eq!(in_values(&one, &[Value::Int(2)], false), Value::Bool(false));
        assert_eq!(in_values(&one, &[Value::Int(2)], true), Value::Bool(true));
        assert_eq!(
            in_values(&Value::Null, &[Value::Int(1)], false),
            Value::Null
        );
    }

    #[test]
    fn arithmetic_rules() {
        assert_eq!(
            arith(BinaryOp::Divide, &Value::Int(1), &Value::Int(2)).unwrap(),
            Value::Float(0.5)
        );
        assert_eq!(
            arith(BinaryOp::Divide, &Value::Int(1), &Value::Int(0)).unwrap(),
            Value::Null
        );
        assert_eq!(
            arith(BinaryOp::Plus, &Value::Int(1), &Value::Float(1.5)).unwrap(),
            Value::Float(2.5)
        );
        assert!(arith(BinaryOp::Plus, &Value::Int(i64::MAX), &Value::Int(1)).is_err());
        assert!(arith(BinaryOp::Modulo, &Value::Int(1), &Value::Int(0)).is_err());
        assert_eq!(
            arith(BinaryOp::Plus, &Value::Null, &Value::Int(1)).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn comparison_rules() {
        assert_eq!(
            compare(BinaryOp::Lt, &Value::Int(1), &Value::Float(1.5)).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            compare(BinaryOp::Eq, &Value::from("a"), &Value::Int(1)).unwrap(),
            Value::Bool(false)
        );
        assert!(compare(BinaryOp::Lt, &Value::from("a"), &Value::Int(1)).is_err());
        assert_eq!(
            compare(BinaryOp::Eq, &Value::Null, &Value::Null).unwrap(),
            Value::Null
        );
    }
}
