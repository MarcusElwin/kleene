//! CallSQL frontend.
//!
//! Turns SQL text into a [`LogicalPlan`] in three steps: parse with
//! `sqlparser` (PostgreSQL dialect), validate against the CallSQL subset, and
//! resolve names and types against a [`Catalog`]. Anything outside the subset
//! is a [`SqlError`] carrying a hint, which the harness renders back to the
//! model as a result row.
//!
//! This crate is pure: no I/O, no async.

#![forbid(unsafe_code)]

pub mod error;
pub mod expr;
mod extensions;
pub mod plan;
mod planner;
mod scope;
mod similar;
pub mod statement;
pub mod types;

pub use callgebra_core::Catalog;
pub use error::SqlError;
pub use expr::{AggregateFn, BinaryOp, Expr, Literal, UnaryOp};
pub use extensions::{plan_call, plan_create_agent, plan_create_function};
pub use plan::{JoinKind, LogicalPlan, SortKey};
pub use statement::{FunctionBody, Statement, StatementKind};
pub use types::type_of;

/// Parse one or more CallSQL statements from text.
///
/// Parsing is syntax-only; call [`plan()`] or [`plan_sql()`] to validate and
/// resolve.
pub fn parse(sql: &str) -> Result<Vec<sqlparser::ast::Statement>, SqlError> {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;
    Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|e| {
        let message = e.to_string();
        let hint = parse_hint(sql);
        SqlError::Parse { message, hint }
    })
}

/// Validate a parsed statement against the CallSQL subset and resolve it
/// against the catalog, producing a [`Statement`] ready for the planner.
pub fn plan(stmt: &sqlparser::ast::Statement, catalog: &Catalog) -> Result<Statement, SqlError> {
    planner::Planner::new(catalog).plan_statement(stmt, stmt.to_string())
}

/// Parse, validate and resolve every statement in `sql`.
///
/// `FINAL(expr)`, `FINAL FROM (query)`, `CREATE FUNCTION ... AS PROMPT/SQL/SHELL`
/// and `CALL tool(args) [FROM query]` are recognised here before parsing,
/// because they are CallSQL additions the parser does not know. Such a
/// statement must be the only statement in the text.
pub fn plan_sql(sql: &str, catalog: &Catalog) -> Result<Vec<Statement>, SqlError> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if let Some(rest) = strip_keyword(trimmed, "FINAL") {
        let rest = rest.trim();
        let rewritten = if let Some(q) = strip_keyword(rest, "FROM") {
            format!("SELECT * FROM {} AS final", q.trim())
        } else {
            format!("SELECT {} AS answer", rest)
        };
        let stmts = parse(&rewritten)?;
        let [only] = stmts.as_slice() else {
            return Err(SqlError::Parse {
                message: "FINAL takes one expression or one query".into(),
                hint: Some("FINAL(expr) or FINAL FROM (SELECT ...)".into()),
            });
        };
        let planned = plan(only, catalog)?;
        let StatementKind::Query { plan } = planned.kind else {
            unreachable!("FINAL rewrites to a SELECT");
        };
        return Ok(vec![Statement {
            sql: sql.trim().to_string(),
            kind: StatementKind::Final { plan },
        }]);
    }
    if let Some(stmt) = extensions::plan_extension(trimmed, catalog)? {
        return Ok(vec![stmt]);
    }
    let stmts = parse(sql)?;
    let single = stmts.len() == 1;
    let planner = planner::Planner::new(catalog);
    stmts
        .iter()
        .map(|s| {
            let text = if single {
                sql.trim().to_string()
            } else {
                s.to_string()
            };
            planner.plan_statement(s, text)
        })
        .collect()
}

/// One-line rendering of an error for the model: `error: ...` plus an
/// optional `hint: ...` line.
pub fn render_error(err: &SqlError) -> String {
    match err.hint() {
        Some(h) => format!("error: {err}\nhint: {h}"),
        None => format!("error: {err}"),
    }
}

fn strip_keyword<'a>(text: &'a str, kw: &str) -> Option<&'a str> {
    let head: String = text.chars().take(kw.len()).collect();
    if head.eq_ignore_ascii_case(kw) {
        let rest = &text[head.len()..];
        if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '(') {
            return Some(rest);
        }
    }
    None
}

fn parse_hint(sql: &str) -> Option<String> {
    let upper = sql.to_ascii_uppercase();
    if upper.contains("SELEC ") {
        return Some("did you mean SELECT?".into());
    }
    if upper.contains(" FORM ") {
        return Some("did you mean FROM?".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recursive_cte_with_lateral_and_exists() {
        let sql = r#"
            WITH RECURSIVE frontier AS (
                SELECT hypothesis, 0 AS d FROM seeds
                UNION ALL
                SELECT e.item, d + 1 FROM frontier
                CROSS JOIN LATERAL expand(hypothesis, 3) AS e
                WHERE d < 2
            )
            SELECT hypothesis FROM frontier f
            WHERE verify(hypothesis)
              AND NOT EXISTS (SELECT 1 FROM counterexamples c WHERE refute(f.hypothesis, c.text));
        "#;
        let stmts = parse(sql).expect("parses");
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn parse_errors_carry_a_message_and_hint() {
        let err = parse("SELEC 1").unwrap_err();
        assert!(matches!(err, SqlError::Parse { .. }));
        assert_eq!(err.hint(), Some("did you mean SELECT?"));
    }
}
