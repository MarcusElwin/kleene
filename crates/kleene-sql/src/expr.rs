//! Resolved scalar expressions.

use kleene_core::{DataType, Value};
use serde::{Deserialize, Serialize};

/// A literal constant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Literal(pub Value);

/// Binary operators in the subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    /// `=`
    Eq,
    /// `<>`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `AND`
    And,
    /// `OR`
    Or,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Modulo,
    /// `||`
    Concat,
    /// `LIKE`
    Like,
    /// `ILIKE`
    ILike,
}

/// Unary operators in the subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    /// `NOT`
    Not,
    /// `-`
    Neg,
    /// `IS NULL`
    IsNull,
    /// `IS NOT NULL`
    IsNotNull,
}

/// Aggregate functions in the subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFn {
    /// `COUNT(*)` or `COUNT(expr)`
    Count,
    /// `SUM`
    Sum,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
    /// `AVG`
    Avg,
    /// `STRING_AGG(expr, sep)`
    StringAgg,
    /// `BOOL_AND`
    BoolAnd,
    /// `BOOL_OR`
    BoolOr,
}

/// A resolved expression. Column references are by position in the input
/// schema, so evaluation never looks names up.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "expr")]
pub enum Expr {
    /// Input column by position.
    Column {
        /// Position in the operator's input schema.
        index: usize,
        /// Name, kept for EXPLAIN and error messages.
        name: String,
    },
    /// A constant.
    Literal(Literal),
    /// Binary operation.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand.
        operand: Box<Expr>,
    },
    /// Scalar function call, resolved against the catalog. This is where
    /// `LLM(...)`, `VERIFY(...)` and other call functions appear.
    Function {
        /// Catalog name (lower-cased).
        name: String,
        /// Arguments.
        args: Vec<Expr>,
        /// Result type from the catalog.
        data_type: DataType,
    },
    /// Aggregate call; only valid directly under an aggregate operator.
    Aggregate {
        /// Which aggregate.
        func: AggregateFn,
        /// Arguments (empty for `COUNT(*)`).
        args: Vec<Expr>,
        /// `DISTINCT`.
        distinct: bool,
    },
    /// `CAST(expr AS type)`.
    Cast {
        /// Operand.
        operand: Box<Expr>,
        /// Target type.
        to: DataType,
    },
    /// `CASE WHEN ... THEN ... ELSE ... END`.
    Case {
        /// `(condition, result)` pairs in order.
        branches: Vec<(Expr, Expr)>,
        /// `ELSE` result.
        otherwise: Option<Box<Expr>>,
    },
    /// `[NOT] EXISTS (subquery)`. The subquery may reference outer columns
    /// through [`Expr::OuterColumn`].
    Exists {
        /// The subquery plan.
        subquery: Box<crate::plan::LogicalPlan>,
        /// `NOT EXISTS`.
        negated: bool,
    },
    /// `expr [NOT] IN (subquery)`.
    InSubquery {
        /// Left operand.
        operand: Box<Expr>,
        /// Single-column subquery.
        subquery: Box<crate::plan::LogicalPlan>,
        /// `NOT IN`.
        negated: bool,
    },
    /// Scalar subquery producing one value.
    ScalarSubquery {
        /// Single-row, single-column subquery.
        subquery: Box<crate::plan::LogicalPlan>,
    },
    /// Reference to a column of an enclosing query, for correlated subqueries.
    OuterColumn {
        /// How many query levels up.
        depth: usize,
        /// Position in that level's input schema.
        index: usize,
        /// Name, for EXPLAIN.
        name: String,
    },
    /// `expr IN (v1, v2, ...)` with literal or scalar list members.
    InList {
        /// Left operand.
        operand: Box<Expr>,
        /// Candidates.
        list: Vec<Expr>,
        /// `NOT IN`.
        negated: bool,
    },
}
