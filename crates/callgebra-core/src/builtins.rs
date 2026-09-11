//! The standard library of pure functions every session sees.
//!
//! The SQL frontend types calls against these definitions and the executor
//! evaluates them; both sides read this one list so they cannot drift.
//! Semantics follow DuckDB, which is the differential-test oracle.

use crate::catalog::{CallKind, FunctionDef, FunctionReturn, Volatility};
use crate::value::{DataType, Field, Schema};

fn scalar(
    name: &str,
    args: &[DataType],
    variadic: bool,
    returns: DataType,
    description: &str,
) -> FunctionDef {
    FunctionDef {
        name: name.to_string(),
        args: args.to_vec(),
        variadic,
        returns: FunctionReturn::Scalar { data_type: returns },
        call_kind: CallKind::Pure,
        volatility: Volatility::Immutable,
        description: description.to_string(),
    }
}

/// Pure scalar and table functions available in every catalog.
///
/// Scalar functions (all `IMMUTABLE`, all `NULL` in gives `NULL` out unless noted):
///
/// | name | signature | notes |
/// |---|---|---|
/// | `upper`, `lower`, `trim`, `ltrim`, `rtrim` | `(TEXT) -> TEXT` | |
/// | `length` | `(TEXT) -> BIGINT` | characters, not bytes |
/// | `substr` | `(TEXT, BIGINT [, BIGINT]) -> TEXT` | 1-based start, optional length |
/// | `concat` | `(ANY, ...) -> TEXT` | `NULL` args are skipped |
/// | `replace` | `(TEXT, TEXT, TEXT) -> TEXT` | |
/// | `starts_with`, `contains` | `(TEXT, TEXT) -> BOOLEAN` | |
/// | `split_part` | `(TEXT, TEXT, BIGINT) -> TEXT` | 1-based; empty string when out of range |
/// | `coalesce` | `(ANY, ...) -> ANY` | first non-`NULL` |
/// | `nullif` | `(ANY, ANY) -> ANY` | |
/// | `greatest`, `least` | `(ANY, ...) -> ANY` | `NULL`s ignored |
/// | `abs` | `(DOUBLE) -> DOUBLE` | keeps `BIGINT` for integer input |
/// | `round` | `(DOUBLE [, BIGINT]) -> DOUBLE` | |
/// | `floor`, `ceil` | `(DOUBLE) -> DOUBLE` | |
/// | `mod` | `(BIGINT, BIGINT) -> BIGINT` | |
/// | `json_extract` | `(JSON, TEXT) -> JSON` | path like `$.a.b[0]` |
/// | `json_extract_string` | `(JSON, TEXT) -> TEXT` | |
///
/// Table functions:
///
/// | name | signature | columns |
/// |---|---|---|
/// | `generate_series` | `(BIGINT, BIGINT [, BIGINT])` | `generate_series BIGINT`, inclusive bounds |
pub fn standard_functions() -> Vec<FunctionDef> {
    use DataType::*;
    let mut v = vec![
        scalar("upper", &[Text], false, Text, "upper-case a string"),
        scalar("lower", &[Text], false, Text, "lower-case a string"),
        scalar(
            "trim",
            &[Text],
            false,
            Text,
            "strip leading and trailing whitespace",
        ),
        scalar("ltrim", &[Text], false, Text, "strip leading whitespace"),
        scalar("rtrim", &[Text], false, Text, "strip trailing whitespace"),
        scalar("length", &[Text], false, Int, "number of characters"),
        scalar(
            "substr",
            &[Text, Int],
            true,
            Text,
            "substring from a 1-based start, optional length",
        ),
        scalar(
            "concat",
            &[Any],
            true,
            Text,
            "concatenate values as text, skipping NULLs",
        ),
        scalar(
            "replace",
            &[Text, Text, Text],
            false,
            Text,
            "replace every occurrence",
        ),
        scalar("starts_with", &[Text, Text], false, Bool, "prefix test"),
        scalar("contains", &[Text, Text], false, Bool, "substring test"),
        scalar(
            "split_part",
            &[Text, Text, Int],
            false,
            Text,
            "1-based field of a split string",
        ),
        scalar("coalesce", &[Any], true, Any, "first non-NULL argument"),
        scalar(
            "nullif",
            &[Any, Any],
            false,
            Any,
            "NULL if the arguments are equal",
        ),
        scalar(
            "greatest",
            &[Any],
            true,
            Any,
            "largest argument, NULLs ignored",
        ),
        scalar(
            "least",
            &[Any],
            true,
            Any,
            "smallest argument, NULLs ignored",
        ),
        scalar("abs", &[Float], false, Float, "absolute value"),
        scalar(
            "round",
            &[Float],
            true,
            Float,
            "round to optional decimal places",
        ),
        scalar("floor", &[Float], false, Float, "round down"),
        scalar("ceil", &[Float], false, Float, "round up"),
        scalar("mod", &[Int, Int], false, Int, "remainder"),
        scalar(
            "json_extract",
            &[Json, Text],
            false,
            Json,
            "value at a JSON path",
        ),
        scalar(
            "json_extract_string",
            &[Json, Text],
            false,
            Text,
            "text at a JSON path",
        ),
    ];
    v.push(FunctionDef {
        name: "generate_series".into(),
        args: vec![Int, Int],
        variadic: true,
        returns: FunctionReturn::Table {
            schema: Schema::new(vec![Field::not_null("generate_series", Int)]),
        },
        call_kind: CallKind::Pure,
        volatility: Volatility::Immutable,
        description: "integers from start to stop inclusive, optional step".into(),
    });
    v
}

/// A catalog pre-populated with [`standard_functions`].
pub fn standard_catalog() -> crate::catalog::Catalog {
    let mut c = crate::catalog::Catalog::new();
    for f in standard_functions() {
        c.add_function(f);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_catalog_has_every_builtin_once() {
        let c = standard_catalog();
        let n = standard_functions().len();
        assert_eq!(c.functions().count(), n);
        assert!(c.function("UPPER").is_some());
        assert!(matches!(
            c.function("generate_series").unwrap().returns,
            FunctionReturn::Table { .. }
        ));
    }
}
