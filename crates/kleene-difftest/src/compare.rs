//! Result comparison with numeric tolerance and multiset semantics.

use kleene_core::{Row, Value};
use std::cmp::Ordering;

fn close(a: f64, b: f64) -> bool {
    if a == b || (a.is_nan() && b.is_nan()) {
        return true;
    }
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= 1e-9 * scale
}

/// Value equality as the tests see it: NULL equals NULL, integers equal
/// floats numerically, floats within a small relative tolerance.
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(_), _) | (_, Value::Float(_)) | (Value::Int(_), _) | (_, Value::Int(_)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => close(x, y),
                _ => false,
            }
        }
        (Value::Vector(x), Value::Vector(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(p, q)| close(f64::from(*p), f64::from(*q)))
        }
        _ => a == b,
    }
}

fn same_row(a: &Row, b: &Row) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y))
}

/// Canonical ordering used to sort both sides before an unordered comparison.
fn canon_cmp(a: &Row, b: &Row) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = match (x.as_f64(), y.as_f64()) {
            (Some(p), Some(q)) if !x.is_null() && !y.is_null() => p.total_cmp(&q),
            _ => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

fn render(rows: &[Row], limit: usize) -> String {
    rows.iter()
        .take(limit)
        .map(|r| r.iter().map(Value::render).collect::<Vec<_>>().join(" | "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check that `actual` matches `expected`, as lists when `ordered`, else as
/// multisets. The error message shows counts and the first mismatch.
pub fn assert_same(expected: &[Row], actual: &[Row], ordered: bool) -> Result<(), String> {
    if expected.len() != actual.len() {
        return Err(format!(
            "row count differs: expected {} got {}\n-- expected --\n{}\n-- actual --\n{}",
            expected.len(),
            actual.len(),
            render(expected, 10),
            render(actual, 10)
        ));
    }
    let (e, a): (Vec<Row>, Vec<Row>) = if ordered {
        (expected.to_vec(), actual.to_vec())
    } else {
        let mut e = expected.to_vec();
        let mut a = actual.to_vec();
        e.sort_by(canon_cmp);
        a.sort_by(canon_cmp);
        (e, a)
    };
    for (i, (x, y)) in e.iter().zip(&a).enumerate() {
        if !same_row(x, y) {
            return Err(format!(
                "row {i} differs{}:\n  expected: {}\n  actual:   {}\n-- expected --\n{}\n-- actual --\n{}",
                if ordered { "" } else { " (after canonical sort)" },
                render(std::slice::from_ref(x), 1),
                render(std::slice::from_ref(y), 1),
                render(&e, 10),
                render(&a, 10)
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerant_and_multiset() {
        let e = vec![
            vec![Value::Int(1), Value::Float(0.3)],
            vec![Value::Null, Value::from("a")],
        ];
        let a = vec![
            vec![Value::Null, Value::from("a")],
            vec![Value::Float(1.0), Value::Float(0.1 + 0.2)],
        ];
        assert!(assert_same(&e, &a, false).is_ok());
        assert!(assert_same(&e, &a, true).is_err());
        let short = vec![e[0].clone()];
        assert!(assert_same(&e, &short, false)
            .unwrap_err()
            .contains("row count"));
    }
}
