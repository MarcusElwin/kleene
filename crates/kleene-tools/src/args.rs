//! Argument decoding shared by every tool: arity checks, typed accessors and
//! the small formatting helpers (`tokens`, RFC 3339 timestamps) that several
//! tools report.

use crate::{Signature, ToolError};
use kleene_core::Value;
use std::time::SystemTime;

/// Check the argument count against the signature.
pub(crate) fn check_arity(name: &str, sig: &Signature, args: &[Value]) -> Result<(), ToolError> {
    let (min, max, n) = (sig.min_args(), sig.max_args(), args.len());
    if n < min || n > max {
        let want = if min == max {
            format!("{min}")
        } else {
            format!("{min} to {max}")
        };
        return Err(ToolError::Args(format!(
            "{name} takes {want} argument{}, got {n}",
            if max == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

/// A required `TEXT` argument; `NULL` or a non-text value is an error.
pub(crate) fn text<'a>(args: &'a [Value], i: usize, what: &str) -> Result<&'a str, ToolError> {
    match args.get(i) {
        Some(Value::Text(s)) => Ok(s),
        Some(Value::Null) | None => Err(ToolError::Args(format!("{what} must not be NULL"))),
        Some(other) => Err(ToolError::Args(format!(
            "{what} must be TEXT, got {}",
            other.data_type()
        ))),
    }
}

/// An optional `TEXT` argument: absent or `NULL` gives `None`.
pub(crate) fn opt_text<'a>(
    args: &'a [Value],
    i: usize,
    what: &str,
) -> Result<Option<&'a str>, ToolError> {
    match args.get(i) {
        Some(Value::Null) | None => Ok(None),
        _ => text(args, i, what).map(Some),
    }
}

/// A required `BIGINT` argument; floats with no fraction are accepted.
pub(crate) fn int(args: &[Value], i: usize, what: &str) -> Result<i64, ToolError> {
    match args.get(i) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Float(f)) if f.fract() == 0.0 => Ok(*f as i64),
        Some(Value::Null) | None => Err(ToolError::Args(format!("{what} must not be NULL"))),
        Some(other) => Err(ToolError::Args(format!(
            "{what} must be BIGINT, got {}",
            other.data_type()
        ))),
    }
}

/// An optional `BIGINT` argument: absent or `NULL` gives `None`.
pub(crate) fn opt_int(args: &[Value], i: usize, what: &str) -> Result<Option<i64>, ToolError> {
    match args.get(i) {
        Some(Value::Null) | None => Ok(None),
        _ => int(args, i, what).map(Some),
    }
}

/// The token estimate used throughout the surface: `ceil(chars / 4)`.
pub(crate) fn tokens(chars: usize) -> i64 {
    chars.div_ceil(4) as i64
}

/// A timestamp as RFC 3339 in UTC with second precision (`2026-09-11T10:04:05Z`).
pub(crate) fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::DataType;

    #[test]
    fn arity_messages_name_the_range() {
        let sig = Signature::new(&[DataType::Text], &[DataType::Text]);
        let err = check_arity("grep", &sig, &[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid arguments: grep takes 1 to 2 arguments, got 0"
        );
        let sig = Signature::new(&[DataType::Text], &[]);
        let err = check_arity("lines", &sig, &[Value::Null, Value::Null]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid arguments: lines takes 1 argument, got 2"
        );
        assert!(check_arity("lines", &sig, &[Value::Null]).is_ok());
    }

    #[test]
    fn accessors_type_check() {
        let args = [Value::from("a"), Value::Int(3), Value::Null];
        assert_eq!(text(&args, 0, "path").unwrap(), "a");
        assert!(text(&args, 1, "path").is_err());
        assert!(text(&args, 2, "path").is_err());
        assert_eq!(opt_text(&args, 2, "glob").unwrap(), None);
        assert_eq!(opt_text(&args, 9, "glob").unwrap(), None);
        assert_eq!(int(&args, 1, "n").unwrap(), 3);
        assert!(int(&args, 0, "n").is_err());
        assert_eq!(opt_int(&args, 2, "n").unwrap(), None);
        assert_eq!(int(&[Value::Float(2.0)], 0, "n").unwrap(), 2);
        assert!(int(&[Value::Float(2.5)], 0, "n").is_err());
    }

    #[test]
    fn tokens_round_up() {
        assert_eq!(tokens(0), 0);
        assert_eq!(tokens(1), 1);
        assert_eq!(tokens(4), 1);
        assert_eq!(tokens(5), 2);
    }

    #[test]
    fn rfc3339_is_utc_seconds() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(rfc3339(t), "2023-11-14T22:13:20Z");
    }
}
