//! Shared interface types for Callgebra.
//!
//! Every other crate codes against what is defined here: the [`Value`] and
//! [`Batch`] that flow between operators, the [`Schema`] that describes them,
//! the [`Budget`] that bounds a session, and the [`Catalog`] that tells the
//! SQL frontend which relations and functions exist and what each one costs.
//! Nothing in this crate performs I/O.

#![forbid(unsafe_code)]

pub mod batch;
pub mod budget;
pub mod builtins;
pub mod catalog;
pub mod error;
pub mod ids;
pub mod value;

pub use batch::{Batch, Row};
pub use budget::{Budget, BudgetExceeded, BudgetUsage};
pub use builtins::{call_functions, standard_catalog, standard_functions};
pub use catalog::{
    CallKind, Catalog, FunctionDef, FunctionReturn, ModelAlias, ProxySpec, TableDef, TableSource,
    Volatility,
};
pub use error::CoreError;
pub use ids::{CallId, RunId, SessionId, StatementId};
pub use value::{DataType, Field, Schema, Value};
