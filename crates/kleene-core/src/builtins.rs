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

/// The model-call functions every session sees.
///
/// | name | signature | kind |
/// |---|---|---|
/// | `llm` | `(prompt TEXT [, model TEXT, effort TEXT]) -> TEXT` | one call per distinct argument tuple |
/// | `llm_bool` | `(prompt TEXT) -> BOOLEAN` | structured yes/no |
/// | `llm_json` | `(prompt TEXT, schema TEXT) -> JSON` | output validated against the schema |
/// | `expand` | `(prompt TEXT, n BIGINT) -> TABLE(item TEXT)` | up to `n` generated rows |
/// | `rlm` | `(question TEXT [, context TEXT]) -> TABLE(answer TEXT, detail JSON)` | child session at depth + 1 |
/// | `spawn` | `(agent TEXT, task TEXT [, context TEXT]) -> TABLE(answer TEXT, detail JSON, session TEXT)` | declared agent as a child session |
///
/// All are `IMMUTABLE` for memo purposes: the same prompt to the same pinned
/// model is served from the memo. That is a policy, not a fact about models,
/// and the catalog says so.
pub fn call_functions() -> Vec<FunctionDef> {
    use crate::catalog::ModelAlias;
    use DataType::*;
    let worker = || CallKind::LlmScalar {
        alias: ModelAlias::worker(),
    };
    vec![
        FunctionDef {
            name: "llm".into(),
            args: vec![Text],
            variadic: true,
            returns: FunctionReturn::Scalar { data_type: Text },
            call_kind: worker(),
            volatility: Volatility::Immutable,
            description: "ask the worker model; optional model alias and effort".into(),
        },
        FunctionDef {
            name: "llm_bool".into(),
            args: vec![Text],
            variadic: false,
            returns: FunctionReturn::Scalar { data_type: Bool },
            call_kind: worker(),
            volatility: Volatility::Immutable,
            description: "ask the worker model a yes/no question".into(),
        },
        FunctionDef {
            name: "llm_json".into(),
            args: vec![Text, Text],
            variadic: false,
            returns: FunctionReturn::Scalar { data_type: Json },
            call_kind: worker(),
            volatility: Volatility::Immutable,
            description: "ask the worker model for JSON matching a schema".into(),
        },
        FunctionDef {
            name: "expand".into(),
            args: vec![Text, Int],
            variadic: false,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![Field::not_null("item", Text)]),
            },
            call_kind: CallKind::LlmTable {
                alias: ModelAlias::worker(),
            },
            volatility: Volatility::Immutable,
            description: "generate up to n items from a prompt, one row each".into(),
        },
        FunctionDef {
            name: "rlm".into(),
            args: vec![Text],
            variadic: true,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![
                    Field::new("answer", Text),
                    Field::new("detail", Json),
                ]),
            },
            call_kind: CallKind::Recursive {
                role: "self".into(),
            },
            volatility: Volatility::Stable,
            description: "rlm(question [, context]): run a child session at depth+1 with context preloaded as table ctx(text); returns its FINAL rows (answer = first column, detail = the row as JSON)".into(),
        },
        FunctionDef {
            name: "spawn".into(),
            args: vec![Text, Text],
            variadic: true,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![
                    Field::new("answer", Text),
                    Field::new("detail", Json),
                    Field::new("session", Text),
                ]),
            },
            call_kind: CallKind::Recursive {
                role: "agent".into(),
            },
            volatility: Volatility::Stable,
            description: "spawn(agent, task [, context]): run a declared agent (CREATE AGENT) as a child session, one child per input row, concurrently".into(),
        },
    ]
}

/// The typed-decision functions every session sees, answered by the
/// decision provider (TypeSafe's Jev) rather than a text model.
///
/// Each takes a `state` (the content to decide about, text or JSON) and a
/// `question`. Labels and levels are a JSON array or a comma-separated list.
///
/// | name | signature | answer |
/// |---|---|---|
/// | `jev_noul` | `(state TEXT, question TEXT) -> DOUBLE` | probability in `[0, 1]` that the answer is yes |
/// | `jev_choice` | `(state TEXT, question TEXT, labels TEXT) -> TEXT` | the most probable label |
/// | `jev_score` | `(state TEXT, question TEXT, levels TEXT) -> DOUBLE` | expected level, `0` to `n - 1`, fractional between levels |
/// | `jev_choices` | `(state TEXT, question TEXT, labels TEXT) -> TABLE(label TEXT, probability DOUBLE)` | every label with its probability |
///
/// All are `IMMUTABLE` on the same terms as [`call_functions`], and all run
/// on the [`ModelAlias::jev`](crate::ModelAlias::jev) alias, which the cost
/// model prices far below a text call.
pub fn decision_functions() -> Vec<FunctionDef> {
    use crate::catalog::ModelAlias;
    use DataType::*;
    let jev = || CallKind::LlmScalar {
        alias: ModelAlias::jev(),
    };
    vec![
        FunctionDef {
            name: "jev_noul".into(),
            args: vec![Text, Text],
            variadic: false,
            returns: FunctionReturn::Scalar { data_type: Float },
            call_kind: jev(),
            volatility: Volatility::Immutable,
            description: "jev_noul(state, question): probability in [0, 1] that the answer about state is yes; a typed decision, not a text call".into(),
        },
        FunctionDef {
            name: "jev_choice".into(),
            args: vec![Text, Text, Text],
            variadic: false,
            returns: FunctionReturn::Scalar { data_type: Text },
            call_kind: jev(),
            volatility: Volatility::Immutable,
            description: "jev_choice(state, question, labels): the most probable of the labels (JSON array or comma-separated)".into(),
        },
        FunctionDef {
            name: "jev_score".into(),
            args: vec![Text, Text, Text],
            variadic: false,
            returns: FunctionReturn::Scalar { data_type: Float },
            call_kind: jev(),
            volatility: Volatility::Immutable,
            description: "jev_score(state, question, levels): expected level from 0 to n - 1 on the ordered rubric (JSON array or comma-separated)".into(),
        },
        FunctionDef {
            name: "jev_choices".into(),
            args: vec![Text, Text, Text],
            variadic: false,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![
                    Field::not_null("label", Text),
                    Field::not_null("probability", Float),
                ]),
            },
            call_kind: CallKind::LlmTable {
                alias: ModelAlias::jev(),
            },
            volatility: Volatility::Immutable,
            description: "jev_choices(state, question, labels): one row per label with its probability, most probable first".into(),
        },
    ]
}

/// A catalog pre-populated with [`standard_functions`], [`call_functions`]
/// and [`decision_functions`].
pub fn standard_catalog() -> crate::catalog::Catalog {
    let mut c = crate::catalog::Catalog::new();
    for f in standard_functions()
        .into_iter()
        .chain(call_functions())
        .chain(decision_functions())
    {
        c.add_function(f);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ModelAlias;

    #[test]
    fn delegation_functions_are_recursive_table_calls() {
        let c = standard_catalog();
        let rlm = c.function("rlm").unwrap();
        assert!(matches!(rlm.call_kind, CallKind::Recursive { .. }));
        assert!(rlm.variadic && rlm.args.len() == 1);
        let FunctionReturn::Table { schema } = &rlm.returns else {
            panic!()
        };
        assert_eq!(schema.names(), ["answer", "detail"]);
        let spawn = c.function("spawn").unwrap();
        let FunctionReturn::Table { schema } = &spawn.returns else {
            panic!()
        };
        assert_eq!(schema.names(), ["answer", "detail", "session"]);
    }

    #[test]
    fn standard_catalog_has_every_builtin_once() {
        let c = standard_catalog();
        let n = standard_functions().len() + call_functions().len() + decision_functions().len();
        assert_eq!(c.functions().count(), n);
        assert!(matches!(
            c.function("LLM").unwrap().call_kind,
            CallKind::LlmScalar { .. }
        ));
        assert!(c.function("UPPER").is_some());
        assert!(matches!(
            c.function("generate_series").unwrap().returns,
            FunctionReturn::Table { .. }
        ));
    }

    #[test]
    fn decision_functions_run_on_the_jev_alias() {
        let c = standard_catalog();
        for name in ["jev_noul", "jev_choice", "jev_score"] {
            let f = c.function(name).unwrap();
            let CallKind::LlmScalar { alias } = &f.call_kind else {
                panic!("{name} is a scalar decision")
            };
            assert!(alias.is_jev());
            assert_eq!(f.volatility, Volatility::Immutable);
        }
        let FunctionReturn::Table { schema } = &c.function("jev_choices").unwrap().returns else {
            panic!()
        };
        assert_eq!(schema.names(), ["label", "probability"]);
        assert!(!ModelAlias::worker().is_jev());
    }
}
