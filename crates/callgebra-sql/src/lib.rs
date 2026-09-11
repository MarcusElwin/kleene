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
pub mod plan;
pub mod statement;

pub use callgebra_core::Catalog;
pub use error::SqlError;
pub use expr::{AggregateFn, BinaryOp, Expr, Literal, UnaryOp};
pub use plan::{JoinKind, LogicalPlan, SortKey};
pub use statement::{Statement, StatementKind};

/// Parse one or more CallSQL statements from text.
///
/// Parsing is syntax-only; call [`plan()`] to validate and resolve.
pub fn parse(sql: &str) -> Result<Vec<sqlparser::ast::Statement>, SqlError> {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;
    Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|e| SqlError::Parse {
        message: e.to_string(),
        hint: None,
    })
}

/// Validate a parsed statement against the CallSQL subset and resolve it
/// against the catalog, producing a [`Statement`] ready for the planner.
///
/// Implemented in M1.
pub fn plan(_stmt: &sqlparser::ast::Statement, _catalog: &Catalog) -> Result<Statement, SqlError> {
    todo!("M1: subset validation and name resolution")
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
    fn parse_errors_carry_a_message() {
        let err = parse("SELEC 1").unwrap_err();
        assert!(matches!(err, SqlError::Parse { .. }));
    }
}
