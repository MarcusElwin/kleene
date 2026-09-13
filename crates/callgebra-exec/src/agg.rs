//! Aggregate accumulators.

use crate::ExecError;
use callgebra_core::Value;
use callgebra_sql::AggregateFn;
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
    texts: Vec<String>,
    sep: Option<String>,
    all: Option<bool>,
    any: Option<bool>,
}

impl Accumulator {
    pub fn new(func: AggregateFn, distinct: bool) -> Self {
        Self {
            func,
            distinct,
            seen: HashSet::new(),
            count: 0,
            sum: Sum::Empty,
            extreme: None,
            texts: vec![],
            sep: None,
            all: None,
            any: None,
        }
    }

    /// Feed one row's argument values.
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
                self.texts.push(v.render());
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
    pub fn finish(self) -> Value {
        match self.func {
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
                    Value::Text(self.texts.join(self.sep.as_deref().unwrap_or("")))
                }
            }
            AggregateFn::BoolAnd => self.all.map(Value::Bool).unwrap_or(Value::Null),
            AggregateFn::BoolOr => self.any.map(Value::Bool).unwrap_or(Value::Null),
        }
    }
}
