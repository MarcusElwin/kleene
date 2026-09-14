//! Static typing of resolved expressions.

use crate::expr::{AggregateFn, BinaryOp, Expr, UnaryOp};
use kleene_core::{DataType, Schema, Value};

/// The static type of an expression over `input`.
///
/// Types are advisory: `Any` is compatible with everything and the executor
/// re-checks at runtime. Arithmetic on two integers stays `BIGINT` except `/`,
/// which is always `DOUBLE` (DuckDB semantics; DuckDB is the oracle).
pub fn type_of(expr: &Expr, input: &Schema) -> DataType {
    use DataType::*;
    match expr {
        Expr::Column { index, .. } => input.fields.get(*index).map(|f| f.data_type).unwrap_or(Any),
        Expr::Literal(l) => match &l.0 {
            Value::Null => Any,
            v => v.data_type(),
        },
        Expr::Binary { op, left, right } => {
            let (l, r) = (type_of(left, input), type_of(right, input));
            match op {
                BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::Lt
                | BinaryOp::LtEq
                | BinaryOp::Gt
                | BinaryOp::GtEq
                | BinaryOp::And
                | BinaryOp::Or
                | BinaryOp::Like
                | BinaryOp::ILike => Bool,
                BinaryOp::Concat => Text,
                BinaryOp::Divide => Float,
                BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply | BinaryOp::Modulo => {
                    numeric_result(l, r)
                }
            }
        }
        Expr::Unary { op, operand } => match op {
            UnaryOp::Not | UnaryOp::IsNull | UnaryOp::IsNotNull => Bool,
            UnaryOp::Neg => type_of(operand, input),
        },
        Expr::Function {
            data_type,
            name,
            args,
        } => {
            if *data_type == Any {
                // coalesce / nullif / greatest / least take the first typed argument.
                args.iter()
                    .map(|a| type_of(a, input))
                    .find(|t| *t != Any)
                    .unwrap_or(Any)
            } else if name == "abs" {
                args.first().map(|a| type_of(a, input)).unwrap_or(Float)
            } else {
                *data_type
            }
        }
        Expr::Aggregate { func, args, .. } => match func {
            AggregateFn::Count => Int,
            AggregateFn::Avg => Float,
            AggregateFn::StringAgg => Text,
            AggregateFn::BoolAnd | AggregateFn::BoolOr => Bool,
            AggregateFn::Sum | AggregateFn::Min | AggregateFn::Max => {
                args.first().map(|a| type_of(a, input)).unwrap_or(Any)
            }
        },
        Expr::Cast { to, .. } => *to,
        Expr::Case {
            branches,
            otherwise,
        } => branches
            .iter()
            .map(|(_, r)| type_of(r, input))
            .chain(otherwise.iter().map(|o| type_of(o, input)))
            .find(|t| *t != Any)
            .unwrap_or(Any),
        Expr::Exists { .. } | Expr::InSubquery { .. } | Expr::InList { .. } => Bool,
        Expr::ScalarSubquery { subquery } => subquery
            .schema()
            .fields
            .first()
            .map(|f| f.data_type)
            .unwrap_or(Any),
        Expr::OuterColumn { .. } => Any,
    }
}

fn numeric_result(l: DataType, r: DataType) -> DataType {
    use DataType::*;
    match (l, r) {
        (Float, _) | (_, Float) => Float,
        (Int, Int) => Int,
        (Int, Any) | (Any, Int) => Int,
        (Any, Any) => Any,
        _ => Float,
    }
}

/// Whether a type can take part in arithmetic.
pub(crate) fn is_numeric_or_any(t: DataType) -> bool {
    matches!(t, DataType::Int | DataType::Float | DataType::Any)
}

/// Whether a type can be used where a boolean is required.
pub(crate) fn is_bool_or_any(t: DataType) -> bool {
    matches!(t, DataType::Bool | DataType::Any)
}
