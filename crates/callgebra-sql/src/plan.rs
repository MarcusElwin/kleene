//! The logical relational plan.
//!
//! This is ordinary relational algebra with two additions the call algebra
//! needs: table functions (including `LATERAL`) and recursive queries as an
//! explicit fixpoint node.

use crate::expr::Expr;
use callgebra_core::Schema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Join types in the subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    /// `[INNER] JOIN`
    Inner,
    /// `LEFT [OUTER] JOIN`
    Left,
    /// `CROSS JOIN`
    Cross,
    /// Semi-join: keep left rows with a match. Produced by rewriting `EXISTS`.
    Semi,
    /// Anti-semi-join: keep left rows with no match. Produced by rewriting `NOT EXISTS`.
    Anti,
}

/// One `ORDER BY` key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SortKey {
    /// Expression to sort by.
    pub expr: Expr,
    /// Ascending order.
    pub asc: bool,
    /// `NULLS FIRST`.
    pub nulls_first: bool,
}

/// A logical plan node. Every node knows its output schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum LogicalPlan {
    /// Read a catalog table.
    Scan {
        /// Catalog name.
        table: String,
        /// Output schema (after projection pushdown, if any).
        schema: Arc<Schema>,
    },
    /// Literal rows (`VALUES`), also used for the empty relation.
    Values {
        /// Rows as expressions.
        rows: Vec<Vec<Expr>>,
        /// Output schema.
        schema: Arc<Schema>,
    },
    /// A CTE or subquery reference by name.
    CteRef {
        /// CTE name.
        name: String,
        /// Output schema.
        schema: Arc<Schema>,
    },
    /// Compute output columns.
    Project {
        /// Input.
        input: Box<LogicalPlan>,
        /// One expression per output column.
        exprs: Vec<Expr>,
        /// Output schema.
        schema: Arc<Schema>,
    },
    /// Keep rows where the predicate is `TRUE`.
    Filter {
        /// Input.
        input: Box<LogicalPlan>,
        /// Boolean predicate.
        predicate: Expr,
    },
    /// Join two inputs.
    Join {
        /// Left input.
        left: Box<LogicalPlan>,
        /// Right input.
        right: Box<LogicalPlan>,
        /// Kind.
        kind: JoinKind,
        /// `ON` predicate over the concatenated schema; `None` for `CROSS`.
        on: Option<Expr>,
        /// Output schema (left then right; semi/anti: left only).
        schema: Arc<Schema>,
    },
    /// A table function, optionally `LATERAL` over the input.
    TableFunction {
        /// Catalog function name.
        name: String,
        /// Arguments; with `input` present they may reference its columns.
        args: Vec<Expr>,
        /// `LATERAL` input, one call per input row. `None` for a plain `FROM f(...)`.
        input: Option<Box<LogicalPlan>>,
        /// Output schema (input columns then function columns).
        schema: Arc<Schema>,
    },
    /// Group and aggregate.
    Aggregate {
        /// Input.
        input: Box<LogicalPlan>,
        /// Grouping expressions.
        group_by: Vec<Expr>,
        /// Aggregate expressions ([`Expr::Aggregate`]).
        aggregates: Vec<Expr>,
        /// Output schema (groups then aggregates).
        schema: Arc<Schema>,
    },
    /// Sort.
    Sort {
        /// Input.
        input: Box<LogicalPlan>,
        /// Keys.
        keys: Vec<SortKey>,
    },
    /// `LIMIT` / `OFFSET`.
    Limit {
        /// Input.
        input: Box<LogicalPlan>,
        /// Rows to skip.
        offset: usize,
        /// Rows to keep; `None` for no limit.
        limit: Option<usize>,
    },
    /// `DISTINCT`.
    Distinct {
        /// Input.
        input: Box<LogicalPlan>,
    },
    /// `UNION [ALL]` of inputs with identical schemas.
    Union {
        /// Inputs.
        inputs: Vec<LogicalPlan>,
        /// `UNION ALL` keeps duplicates.
        all: bool,
    },
    /// Non-recursive `WITH`: bind CTEs for the body.
    With {
        /// `(name, plan)` in definition order.
        ctes: Vec<(String, LogicalPlan)>,
        /// The query body.
        body: Box<LogicalPlan>,
    },
    /// `WITH RECURSIVE name AS (base UNION [ALL] recursive) body`.
    ///
    /// Evaluated semi-naively: the recursive term sees only the previous
    /// round's delta through [`LogicalPlan::CteRef`].
    Recursive {
        /// CTE name.
        name: String,
        /// Non-recursive term.
        base: Box<LogicalPlan>,
        /// Recursive term, referencing `name`.
        recursive: Box<LogicalPlan>,
        /// `UNION ALL` (bag) versus `UNION` (set, terminates on no new rows).
        all: bool,
        /// Depth bound from `MAXRECURSION`, if given.
        max_rounds: Option<usize>,
        /// Body that uses the CTE.
        body: Box<LogicalPlan>,
    },
}

impl LogicalPlan {
    /// Output schema of this node.
    pub fn schema(&self) -> Arc<Schema> {
        match self {
            LogicalPlan::Scan { schema, .. }
            | LogicalPlan::Values { schema, .. }
            | LogicalPlan::CteRef { schema, .. }
            | LogicalPlan::Project { schema, .. }
            | LogicalPlan::Join { schema, .. }
            | LogicalPlan::TableFunction { schema, .. }
            | LogicalPlan::Aggregate { schema, .. } => schema.clone(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input } => input.schema(),
            LogicalPlan::Union { inputs, .. } => {
                inputs.first().map(LogicalPlan::schema).unwrap_or_default()
            }
            LogicalPlan::With { body, .. } | LogicalPlan::Recursive { body, .. } => body.schema(),
        }
    }

    /// Direct children, for generic tree walks.
    pub fn children(&self) -> Vec<&LogicalPlan> {
        match self {
            LogicalPlan::Scan { .. } | LogicalPlan::Values { .. } | LogicalPlan::CteRef { .. } => {
                vec![]
            }
            LogicalPlan::Project { input, .. }
            | LogicalPlan::Filter { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input } => vec![input],
            LogicalPlan::TableFunction { input, .. } => input.iter().map(|b| &**b).collect(),
            LogicalPlan::Join { left, right, .. } => vec![left, right],
            LogicalPlan::Union { inputs, .. } => inputs.iter().collect(),
            LogicalPlan::With { ctes, body } => {
                let mut v: Vec<&LogicalPlan> = ctes.iter().map(|(_, p)| p).collect();
                v.push(body);
                v
            }
            LogicalPlan::Recursive {
                base,
                recursive,
                body,
                ..
            } => vec![base, recursive, body],
        }
    }
}
