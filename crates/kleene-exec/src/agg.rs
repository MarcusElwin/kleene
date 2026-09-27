//! Aggregate accumulators.

use crate::ExecError;
use kleene_core::Value;
use kleene_sql::{AggregateFn, AggregateOrder};
use std::cmp::Ordering;
use std::collections::HashSet;

enum Sum {
    Empty,
    Int(i64),
    Float(f64),
}

/// One aggregate over one group.
pub(crate) struct Accumulator {
    func: AggregateFn,
    distinct: bool,
    seen: HashSet<Vec<Value>>,
    count: i64,
    sum: Sum,
    extreme: Option<Value>,
    /// `STRING_AGG` pieces with their `ORDER BY` key values.
    texts: Vec<(Vec<Value>, String)>,
    sep: Option<String>,
    order: Vec<AggregateOrder>,
    all: Option<bool>,
    any: Option<bool>,
}

impl Accumulator {
    pub fn new(func: AggregateFn, distinct: bool, order: Vec<AggregateOrder>) -> Self {
        Self {
            func,
            distinct,
            seen: HashSet::new(),
            count: 0,
            sum: Sum::Empty,
            extreme: None,
            texts: vec![],
            sep: None,
            order,
            all: None,
            any: None,
        }
    }

    /// Feed one row's argument values. The last `order.len()` values are the
    /// `ORDER BY` keys of a `STRING_AGG`.
    pub fn push(&mut self, args: &[Value]) -> Result<(), ExecError> {
        if self.func == AggregateFn::Count && args.is_empty() {
            self.count += 1;
            return Ok(());
        }
        let v = args.first().cloned().unwrap_or(Value::Null);
        if v.is_null() {
            return Ok(());
        }
        if self.distinct && !self.seen.insert(vec![v.clone()]) {
            return Ok(());
        }
        match self.func {
            AggregateFn::Count => self.count += 1,
            AggregateFn::Sum | AggregateFn::Avg => {
                self.count += 1;
                self.sum = match (&self.sum, &v) {
                    (Sum::Empty, Value::Int(i)) => Sum::Int(*i),
                    (Sum::Empty, Value::Float(f)) => Sum::Float(*f),
                    (Sum::Int(a), Value::Int(b)) => Sum::Int(
                        a.checked_add(*b)
                            .ok_or_else(|| ExecError::Eval("sum: integer overflow".into()))?,
                    ),
                    (Sum::Int(a), Value::Float(b)) => Sum::Float(*a as f64 + b),
                    (Sum::Float(a), Value::Int(b)) => Sum::Float(a + *b as f64),
                    (Sum::Float(a), Value::Float(b)) => Sum::Float(a + b),
                    (_, other) => {
                        return Err(ExecError::Eval(format!(
                            "sum/avg over non-numeric value {}",
                            other.data_type()
                        )))
                    }
                };
            }
            AggregateFn::Min | AggregateFn::Max => {
                self.extreme = Some(match self.extreme.take() {
                    None => v,
                    Some(cur) => {
                        let ord = v.sql_cmp(&cur).ok_or_else(|| {
                            ExecError::Eval(format!(
                                "min/max: incomparable types {} and {}",
                                v.data_type(),
                                cur.data_type()
                            ))
                        })?;
                        let take = if self.func == AggregateFn::Max {
                            ord.is_gt()
                        } else {
                            ord.is_lt()
                        };
                        if take {
                            v
                        } else {
                            cur
                        }
                    }
                });
            }
            AggregateFn::StringAgg => {
                if self.sep.is_none() {
                    self.sep = Some(match args.get(1) {
                        Some(Value::Text(s)) => s.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(other) => other.render(),
                    });
                }
                let keys = args[args.len().saturating_sub(self.order.len())..].to_vec();
                self.texts.push((keys, v.render()));
            }
            AggregateFn::BoolAnd | AggregateFn::BoolOr => {
                let b = v.as_bool().ok_or_else(|| {
                    ExecError::Eval(format!("bool_and/bool_or over {}", v.data_type()))
                })?;
                self.all = Some(self.all.unwrap_or(true) && b);
                self.any = Some(self.any.unwrap_or(false) || b);
            }
        }
        Ok(())
    }

    /// The aggregate value.
    pub fn finish(self) -> Result<Value, ExecError> {
        Ok(match self.func {
            AggregateFn::Count => Value::Int(self.count),
            AggregateFn::Sum => match self.sum {
                Sum::Empty => Value::Null,
                Sum::Int(i) => Value::Int(i),
                Sum::Float(f) => Value::Float(f),
            },
            AggregateFn::Avg => match self.sum {
                Sum::Empty => Value::Null,
                Sum::Int(i) => Value::Float(i as f64 / self.count as f64),
                Sum::Float(f) => Value::Float(f / self.count as f64),
            },
            AggregateFn::Min | AggregateFn::Max => self.extreme.unwrap_or(Value::Null),
            AggregateFn::StringAgg => {
                if self.texts.is_empty() {
                    Value::Null
                } else {
                    let mut texts = self.texts;
                    if !self.order.is_empty() {
                        let mut error = None;
                        // Stable, so ties keep input order.
                        texts.sort_by(|(a, _), (b, _)| compare_keys(a, b, &self.order, &mut error));
                        if let Some(e) = error {
                            return Err(e);
                        }
                    }
                    let sep = self.sep.as_deref().unwrap_or("");
                    Value::Text(
                        texts
                            .into_iter()
                            .map(|(_, t)| t)
                            .collect::<Vec<_>>()
                            .join(sep),
                    )
                }
            }
            AggregateFn::BoolAnd => self.all.map(Value::Bool).unwrap_or(Value::Null),
            AggregateFn::BoolOr => self.any.map(Value::Bool).unwrap_or(Value::Null),
        })
    }
}

/// Compare two key tuples under the aggregate's `ORDER BY` directions, with
/// the same NULL placement as a top-level `ORDER BY`.
fn compare_keys(
    a: &[Value],
    b: &[Value],
    order: &[AggregateOrder],
    error: &mut Option<ExecError>,
) -> Ordering {
    for (i, o) in order.iter().enumerate() {
        let (x, y) = (&a[i], &b[i]);
        let ord = match (x.is_null(), y.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) => {
                if o.nulls_first {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (false, true) => {
                if o.nulls_first {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (false, false) => match x.sql_cmp(y) {
                Some(c) => {
                    if o.asc {
                        c
                    } else {
                        c.reverse()
                    }
                }
                None => {
                    if error.is_none() {
                        *error = Some(ExecError::Eval(format!(
                            "string_agg ORDER BY: incomparable types {} and {}",
                            x.data_type(),
                            y.data_type()
                        )));
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
