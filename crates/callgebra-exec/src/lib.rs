//! Async pull-based executor.
//!
//! Every operator is a `futures::Stream` of [`Batch`]es. Recursive queries are
//! evaluated semi-naively, one awaitable round at a time. Calls go through
//! the [`CallSink`] supplied by the harness so this crate knows nothing about
//! providers or tools.
//!
//! Implemented in M1 (pure operators, recursion) and M2 (call operators,
//! budgets, memo).

#![forbid(unsafe_code)]

use callgebra_core::{Batch, Budget, BudgetExceeded, BudgetUsage, CoreError, Value};
use callgebra_sql::LogicalPlan;
use futures::stream::BoxStream;
use std::sync::Arc;
use thiserror::Error;

/// Stream of batches produced by an operator.
pub type BatchStream = BoxStream<'static, Result<Batch, ExecError>>;

/// Execution errors.
#[derive(Debug, Error)]
pub enum ExecError {
    /// A budget dimension was exceeded; the statement is cancelled.
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
    /// Core type error (arity, cast).
    #[error(transparent)]
    Core(#[from] CoreError),
    /// A call failed.
    #[error("call failed: {0}")]
    Call(String),
    /// Runtime error in an expression.
    #[error("evaluation error: {0}")]
    Eval(String),
    /// Referenced relation not found in the store.
    #[error("no such table: {0}")]
    NoSuchTable(String),
    /// Statement was cancelled by the user or the harness.
    #[error("cancelled")]
    Cancelled,
}

/// The outside world, as the executor sees it. The harness implements this
/// with providers, tools, the store and the trace; tests implement it with
/// fixtures.
#[async_trait::async_trait]
pub trait CallSink: Send + Sync {
    /// Evaluate a scalar call function on one argument tuple.
    async fn scalar_call(&self, name: &str, args: &[Value]) -> Result<Value, ExecError>;
    /// Evaluate a table function on one argument tuple.
    async fn table_call(&self, name: &str, args: &[Value]) -> Result<Batch, ExecError>;
    /// Read a stored or virtual relation.
    async fn scan(&self, table: &str) -> Result<BatchStream, ExecError>;
}

/// Per-statement execution context.
#[derive(Clone)]
pub struct ExecContext {
    /// Budget for this statement.
    pub budget: Budget,
    /// Shared usage counter, updated by call operators.
    pub usage: Arc<tokio::sync::Mutex<BudgetUsage>>,
    /// Maximum concurrent model calls.
    pub call_concurrency: usize,
    /// Rows per batch.
    pub batch_size: usize,
    /// Cap on semi-naive rounds for recursive queries when the plan sets none.
    pub max_recursion_rounds: Option<usize>,
    /// Where calls go.
    pub sink: Arc<dyn CallSink>,
}

/// Execute a logical plan against a context, producing a stream of batches.
///
/// Implemented in M1.
pub fn execute(_plan: &LogicalPlan, _ctx: ExecContext) -> Result<BatchStream, ExecError> {
    todo!("M1: pull-based operators and semi-naive recursion")
}
