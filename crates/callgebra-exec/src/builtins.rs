//! Native evaluation of the standard function library
//! (`callgebra_core::standard_functions`).

use crate::ExecError;
use callgebra_core::Value;

fn text_arg<'a>(name: &str, args: &'a [Value], i: usize) -> Result<Option<&'a str>, ExecError> {
    match args.get(i) {
        None => Err(ExecError::Eval(format!(
            "{name}: missing argument {}",
            i + 1
        ))),
        Some(Value::Null) => Ok(None),
        Some(Value::Text(s)) => Ok(Some(s)),
        Some(other) => Err(ExecError::Eval(format!(
            "{name}: argument {} must be text, got {}",
            i + 1,
            other.data_type()
        ))),
    }
}

fn int_arg(name: &str, args: &[Value], i: usize) -> Result<Option<i64>, ExecError> {
    match args.get(i) {
        None => Err(ExecError::Eval(format!(
            "{name}: missing argument {}",
            i + 1
        ))),
        Some(Value::Null) => Ok(None),
        Some(Value::Int(n)) => Ok(Some(*n)),
        Some(Value::Float(f)) if f.fract() == 0.0 => Ok(Some(*f as i64)),
        Some(other) => Err(ExecError::Eval(format!(
            "{name}: argument {} must be an integer, got {}",
            i + 1,
            other.data_type()
        ))),
    }
}

fn num_arg(name: &str, args: &[Value], i: usize) -> Result<Option<f64>, ExecError> {
    match args.get(i) {
        None => Err(ExecError::Eval(format!(
            "{name}: missing argument {}",
            i + 1
        ))),
        Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| {
            ExecError::Eval(format!(
                "{name}: argument {} must be numeric, got {}",
                i + 1,
                v.data_type()
            ))
        }),
    }
}

/// `true` if `name` is evaluated natively.
pub(crate) fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "upper"
            | "lower"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "length"
            | "substr"
            | "concat"
            | "replace"
            | "starts_with"
            | "contains"
            | "split_part"
            | "coalesce"
            | "nullif"
            | "greatest"
            | "least"
            | "abs"
            | "round"
            | "floor"
            | "ceil"
            | "mod"
            | "json_extract"
            | "json_extract_string"
    )
}

/// Evaluate a builtin scalar function.
pub(crate) fn call(name: &str, args: &[Value]) -> Result<Value, ExecError> {
    macro_rules! text1 {
        ($f:expr) => {
            Ok(match text_arg(name, args, 0)? {
                None => Value::Null,
                Some(s) => Value::Text($f(s)),
            })
        };
    }
    match name {
        "upper" => text1!(|s: &str| s.to_uppercase()),
        "lower" => text1!(|s: &str| s.to_lowercase()),
        "trim" => text1!(|s: &str| s.trim().to_string()),
        "ltrim" => text1!(|s: &str| s.trim_start().to_string()),
        "rtrim" => text1!(|s: &str| s.trim_end().to_string()),
        "length" => Ok(match text_arg(name, args, 0)? {
            None => Value::Null,
            Some(s) => Value::Int(s.chars().count() as i64),
        }),
        "substr" => {
            let (Some(s), Some(start)) = (text_arg(name, args, 0)?, int_arg(name, args, 1)?) else {
                return Ok(Value::Null);
            };
            let len = if args.len() > 2 {
                match int_arg(name, args, 2)? {
                    None => return Ok(Value::Null),
                    Some(l) => Some(l),
                }
            } else {
                None
            };
            // PostgreSQL semantics: positions start at 1; a start below 1 eats into the length.
            let chars: Vec<char> = s.chars().collect();
            let (from, to) = match len {
                Some(l) => {
                    if l < 0 {
                        return Err(ExecError::Eval("substr: negative length".into()));
                    }
                    let end = start.saturating_add(l); // exclusive, 1-based
                    (start.max(1), end.max(1))
                }
                None => (start.max(1), i64::MAX),
            };
            let from0 = (from - 1) as usize;
            let to0 = if to == i64::MAX {
                chars.len()
            } else {
                ((to - 1) as usize).min(chars.len())
            };
            Ok(Value::Text(if from0 >= to0 {
                String::new()
            } else {
                chars[from0..to0].iter().collect()
            }))
        }
        "concat" => Ok(Value::Text(
            args.iter()
                .filter(|v| !v.is_null())
                .map(Value::render)
                .collect::<Vec<_>>()
                .join(""),
        )),
        "replace" => {
            let (Some(s), Some(from), Some(to)) = (
                text_arg(name, args, 0)?,
                text_arg(name, args, 1)?,
                text_arg(name, args, 2)?,
            ) else {
                return Ok(Value::Null);
            };
            Ok(Value::Text(if from.is_empty() {
                s.to_string()
            } else {
                s.replace(from, to)
            }))
        }
        "starts_with" => {
            let (Some(s), Some(p)) = (text_arg(name, args, 0)?, text_arg(name, args, 1)?) else {
                return Ok(Value::Null);
            };
            Ok(Value::Bool(s.starts_with(p)))
        }
        "contains" => {
            let (Some(s), Some(p)) = (text_arg(name, args, 0)?, text_arg(name, args, 1)?) else {
                return Ok(Value::Null);
            };
            Ok(Value::Bool(s.contains(p)))
        }
        "split_part" => {
            let (Some(s), Some(sep), Some(n)) = (
                text_arg(name, args, 0)?,
                text_arg(name, args, 1)?,
                int_arg(name, args, 2)?,
            ) else {
                return Ok(Value::Null);
            };
            if n < 1 {
                return Err(ExecError::Eval("split_part: index must be >= 1".into()));
            }
            let part = if sep.is_empty() {
                if n == 1 {
                    s
                } else {
                    ""
                }
            } else {
                s.split(sep).nth((n - 1) as usize).unwrap_or("")
            };
            Ok(Value::Text(part.to_string()))
        }
        "coalesce" => Ok(args
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null)),
        "nullif" => {
            let (a, b) = (
                args.first().cloned().unwrap_or(Value::Null),
                args.get(1).cloned().unwrap_or(Value::Null),
            );
            Ok(match a.sql_eq(&b) {
                Some(true) => Value::Null,
                _ => a,
            })
        }
        "greatest" | "least" => {
            let mut best: Option<&Value> = None;
            for v in args.iter().filter(|v| !v.is_null()) {
                best = Some(match best {
                    None => v,
                    Some(b) => {
                        let ord = v.sql_cmp(b).ok_or_else(|| {
                            ExecError::Eval(format!(
                                "{name}: incomparable types {} and {}",
                                v.data_type(),
                                b.data_type()
                            ))
                        })?;
                        let take = if name == "greatest" {
                            ord.is_gt()
                        } else {
                            ord.is_lt()
                        };
                        if take {
                            v
                        } else {
                            b
                        }
                    }
                });
            }
            Ok(best.cloned().unwrap_or(Value::Null))
        }
        "abs" => Ok(match args.first() {
            Some(Value::Int(i)) => Value::Int(
                i.checked_abs()
                    .ok_or_else(|| ExecError::Eval("abs: overflow".into()))?,
            ),
            Some(Value::Float(f)) => Value::Float(f.abs()),
            Some(Value::Null) | None => Value::Null,
            Some(other) => {
                return Err(ExecError::Eval(format!(
                    "abs: expected a number, got {}",
                    other.data_type()
                )))
            }
        }),
        "round" => {
            let Some(x) = num_arg(name, args, 0)? else {
                return Ok(Value::Null);
            };
            let places = if args.len() > 1 {
                int_arg(name, args, 1)?.unwrap_or(0)
            } else {
                0
            };
            let factor = 10f64.powi(places as i32);
            let rounded = (x * factor).round() / factor;
            Ok(match args.first() {
                Some(Value::Int(_)) if places >= 0 => Value::Int(rounded as i64),
                _ => Value::Float(rounded),
            })
        }
        "floor" => Ok(match num_arg(name, args, 0)? {
            None => Value::Null,
            Some(x) => match args.first() {
                Some(Value::Int(i)) => Value::Int(*i),
                _ => Value::Float(x.floor()),
            },
        }),
        "ceil" => Ok(match num_arg(name, args, 0)? {
            None => Value::Null,
            Some(x) => match args.first() {
                Some(Value::Int(i)) => Value::Int(*i),
                _ => Value::Float(x.ceil()),
            },
        }),
        "mod" => {
            let (Some(a), Some(b)) = (int_arg(name, args, 0)?, int_arg(name, args, 1)?) else {
                return Ok(Value::Null);
            };
            if b == 0 {
                return Ok(Value::Null);
            }
            Ok(Value::Int(a.wrapping_rem(b)))
        }
        "json_extract" | "json_extract_string" => {
            let doc = match args.first() {
                Some(Value::Json(j)) => j.clone(),
                Some(Value::Text(s)) => serde_json::from_str(s)
                    .map_err(|e| ExecError::Eval(format!("{name}: invalid JSON: {e}")))?,
                Some(Value::Null) | None => return Ok(Value::Null),
                Some(other) => {
                    return Err(ExecError::Eval(format!(
                        "{name}: expected JSON, got {}",
                        other.data_type()
                    )))
                }
            };
            let Some(path) = text_arg(name, args, 1)? else {
                return Ok(Value::Null);
            };
            let found = json_path(&doc, path)?;
            Ok(match found {
                None => Value::Null,
                Some(v) if name == "json_extract_string" => match v {
                    serde_json::Value::String(s) => Value::Text(s.clone()),
                    serde_json::Value::Null => Value::Null,
                    other => Value::Text(other.to_string()),
                },
                Some(v) => Value::Json(v.clone()),
            })
        }
        other => Err(ExecError::Eval(format!("unknown builtin {other}"))),
    }
}

/// Resolve a path like `$.a.b[0]` or `a.b[0]`.
fn json_path<'a>(
    doc: &'a serde_json::Value,
    path: &str,
) -> Result<Option<&'a serde_json::Value>, ExecError> {
    let mut cur = doc;
    let p = path.strip_prefix('$').unwrap_or(path);
    for seg in p.split('.').filter(|s| !s.is_empty()) {
        let (key, indexes) = match seg.find('[') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if !key.is_empty() {
            cur = match cur.get(key) {
                Some(v) => v,
                None => return Ok(None),
            };
        }
        for idx in indexes.split(']').filter(|s| !s.is_empty()) {
            let n: usize = idx
                .trim_start_matches('[')
                .parse()
                .map_err(|_| ExecError::Eval(format!("bad JSON path segment {seg}")))?;
            cur = match cur.get(n) {
                Some(v) => v,
                None => return Ok(None),
            };
        }
    }
    Ok(Some(cur))
}

/// Rows of `generate_series(start, stop [, step])`, inclusive.
pub(crate) fn generate_series(args: &[Value]) -> Result<Vec<Vec<Value>>, ExecError> {
    let name = "generate_series";
    let (start, stop) = match (int_arg(name, args, 0)?, int_arg(name, args, 1)?) {
        (Some(a), Some(b)) => (a, b),
        _ => return Ok(vec![]),
    };
    let step = if args.len() > 2 {
        int_arg(name, args, 2)?.unwrap_or(1)
    } else {
        1
    };
    if step == 0 {
        return Err(ExecError::Eval(
            "generate_series: step cannot be zero".into(),
        ));
    }
    let mut out = vec![];
    let mut cur = start;
    while (step > 0 && cur <= stop) || (step < 0 && cur >= stop) {
        out.push(vec![Value::Int(cur)]);
        cur = match cur.checked_add(step) {
            Some(c) => c,
            None => break,
        };
        if out.len() > 10_000_000 {
            return Err(ExecError::Eval("generate_series: too many rows".into()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::from(s)
    }

    #[test]
    fn string_functions() {
        assert_eq!(call("upper", &[t("ab")]).unwrap(), t("AB"));
        assert_eq!(call("lower", &[Value::Null]).unwrap(), Value::Null);
        assert_eq!(call("length", &[t("héllo")]).unwrap(), Value::Int(5));
        assert_eq!(
            call("substr", &[t("hello"), Value::Int(2), Value::Int(3)]).unwrap(),
            t("ell")
        );
        assert_eq!(
            call("substr", &[t("hello"), Value::Int(0), Value::Int(3)]).unwrap(),
            t("he")
        );
        assert_eq!(
            call("substr", &[t("hello"), Value::Int(4)]).unwrap(),
            t("lo")
        );
        assert_eq!(
            call("concat", &[t("a"), Value::Null, Value::Int(1)]).unwrap(),
            t("a1")
        );
        assert_eq!(
            call("replace", &[t("aXbX"), t("X"), t("-")]).unwrap(),
            t("a-b-")
        );
        assert_eq!(
            call("starts_with", &[t("abc"), t("ab")]).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            call("contains", &[t("abc"), t("z")]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            call("split_part", &[t("a,b,c"), t(","), Value::Int(2)]).unwrap(),
            t("b")
        );
        assert_eq!(
            call("split_part", &[t("a,b,c"), t(","), Value::Int(9)]).unwrap(),
            t("")
        );
        assert_eq!(call("trim", &[t("  x ")]).unwrap(), t("x"));
    }

    #[test]
    fn null_functions_and_numbers() {
        assert_eq!(
            call("coalesce", &[Value::Null, Value::Int(2), Value::Int(3)]).unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            call("nullif", &[Value::Int(1), Value::Int(1)]).unwrap(),
            Value::Null
        );
        assert_eq!(
            call("nullif", &[Value::Int(1), Value::Int(2)]).unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            call("greatest", &[Value::Int(1), Value::Null, Value::Float(2.5)]).unwrap(),
            Value::Float(2.5)
        );
        assert_eq!(
            call("least", &[Value::Int(1), Value::Float(2.5)]).unwrap(),
            Value::Int(1)
        );
        assert_eq!(call("greatest", &[Value::Null]).unwrap(), Value::Null);
        assert_eq!(call("abs", &[Value::Int(-3)]).unwrap(), Value::Int(3));
        assert_eq!(
            call("round", &[Value::Float(2.5)]).unwrap(),
            Value::Float(3.0)
        );
        assert_eq!(
            call("round", &[Value::Float(2.345), Value::Int(2)]).unwrap(),
            Value::Float(2.35)
        );
        assert_eq!(
            call("floor", &[Value::Float(-1.5)]).unwrap(),
            Value::Float(-2.0)
        );
        assert_eq!(
            call("ceil", &[Value::Float(1.2)]).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(
            call("mod", &[Value::Int(7), Value::Int(3)]).unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            call("mod", &[Value::Int(7), Value::Int(0)]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn json_functions() {
        let doc = Value::Json(serde_json::json!({"a": {"b": [10, {"c": "x"}]}}));
        assert_eq!(
            call("json_extract", &[doc.clone(), t("$.a.b[0]")]).unwrap(),
            Value::Json(serde_json::json!(10))
        );
        assert_eq!(
            call("json_extract_string", &[doc.clone(), t("a.b[1].c")]).unwrap(),
            t("x")
        );
        assert_eq!(
            call("json_extract", &[doc, t("$.zz")]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn series() {
        let rows = generate_series(&[Value::Int(1), Value::Int(3)]).unwrap();
        assert_eq!(rows.len(), 3);
        let rows = generate_series(&[Value::Int(5), Value::Int(1), Value::Int(-2)]).unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r[0].as_int().unwrap())
                .collect::<Vec<_>>(),
            [5, 3, 1]
        );
        assert!(generate_series(&[Value::Int(1), Value::Int(3), Value::Int(0)]).is_err());
        assert!(generate_series(&[Value::Int(3), Value::Int(1)])
            .unwrap()
            .is_empty());
    }
}
