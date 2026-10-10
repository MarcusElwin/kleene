//! Async pull-based executor.
//!
//! Every operator produces a stream of [`Batch`]es. Recursive queries are
//! evaluated semi-naively, one round at a time. Calls go through the
//! [`CallSink`] supplied by the harness so this crate knows nothing about
//! providers or tools.
//!
//! In M1 operators materialise their input (each pulls its whole input into a
//! `Vec<Row>`); the stream boundary is kept so operators can become streaming
//! one at a time later. M2 adds call operators, budgets and the memo.

#![forbid(unsafe_code)]

mod agg;
mod builtins;
mod eval;
mod like;
pub mod memory;
mod ops;

use futures::stream::BoxStream;
use futures::StreamExt;
use kleene_core::{Batch, Budget, BudgetExceeded, BudgetUsage, CoreError, Row, Schema, Value};
use kleene_sql::{LogicalPlan, Statement, StatementKind};
use std::sync::Arc;
use thiserror::Error;

pub use memory::MemorySink;

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
    /// A table already exists.
    #[error("table already exists: {0}")]
    TableExists(String),
    /// Statement was cancelled by the user or the harness.
    #[error("cancelled")]
    Cancelled,
}

/// The outside world, as the executor sees it. The harness implements this
/// with providers, tools, the store and the trace; tests implement it with
/// fixtures.
#[async_trait::async_trait]
pub trait CallSink: Send + Sync {
    /// Evaluate a scalar call function on one argument tuple. Standard
    /// builtins never reach here; the executor evaluates them itself.
    async fn scalar_call(&self, name: &str, args: &[Value]) -> Result<Value, ExecError>;
    /// How many argument tuples `name` can answer in one call, when its
    /// definition is batchable (`CREATE FUNCTION ... BATCH n`). `None`, the
    /// default, means one call per tuple and the executor never batches.
    async fn batch_size(&self, _name: &str) -> Option<usize> {
        None
    }
    /// Evaluate a scalar call function on many argument tuples at once,
    /// returning one value per tuple in order. The executor calls this with
    /// at most `batch_size(name)` distinct, null-free tuples; the default
    /// falls back to one `scalar_call` per tuple.
    async fn scalar_call_batch(
        &self,
        name: &str,
        tuples: &[Vec<Value>],
    ) -> Result<Vec<Value>, ExecError> {
        let mut out = Vec::with_capacity(tuples.len());
        for t in tuples {
            out.push(self.scalar_call(name, t).await?);
        }
        Ok(out)
    }
    /// Evaluate a table function on one argument tuple (`generate_series`
    /// is evaluated natively and never reaches here).
    async fn table_call(&self, name: &str, args: &[Value]) -> Result<Batch, ExecError>;
    /// Read a stored or virtual relation.
    async fn scan(&self, table: &str) -> Result<BatchStream, ExecError>;
    /// Create an empty table. With `if_not_exists`, an existing table is left
    /// alone and `Ok(false)` is returned; `Ok(true)` means the table was created.
    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError>;
    /// Append rows to a table.
    async fn insert(&self, name: &str, batch: Batch) -> Result<(), ExecError>;
    /// Drop a table. With `if_exists`, a missing table is not an error.
    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError>;
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

impl ExecContext {
    /// A context with an unbounded budget and default sizes over `sink`.
    pub fn new(sink: Arc<dyn CallSink>) -> Self {
        Self {
            budget: Budget::unbounded(),
            usage: Arc::new(tokio::sync::Mutex::new(BudgetUsage::default())),
            call_concurrency: 8,
            batch_size: 1024,
            max_recursion_rounds: None,
            sink,
        }
    }
}

/// Hard cap on semi-naive rounds regardless of configuration.
pub const RECURSION_HARD_CAP: usize = 10_000;

/// Execute a logical plan, producing a stream of batches.
///
/// The plan is evaluated eagerly into rows and then chunked by
/// `ctx.batch_size`; see the crate docs for why.
pub async fn execute(plan: &LogicalPlan, ctx: ExecContext) -> Result<BatchStream, ExecError> {
    let schema = plan.schema();
    let rows = ops::eval_plan(plan, &ops::Env::default(), &ctx).await?;
    Ok(chunk(schema, rows, ctx.batch_size))
}

/// Chunk rows into a batch stream.
pub fn chunk(schema: Arc<Schema>, rows: Vec<Row>, batch_size: usize) -> BatchStream {
    let size = batch_size.max(1);
    let mut batches: Vec<Result<Batch, ExecError>> = Vec::new();
    if rows.is_empty() {
        batches.push(Ok(Batch::empty(schema)));
    } else {
        let mut it = rows.into_iter().peekable();
        while it.peek().is_some() {
            let chunk: Vec<Row> = it.by_ref().take(size).collect();
            batches.push(Batch::try_new(schema.clone(), chunk).map_err(ExecError::from));
        }
    }
    Box::pin(futures::stream::iter(batches))
}

/// Drain a stream into one batch.
pub async fn collect(mut stream: BatchStream) -> Result<Batch, ExecError> {
    let mut out: Option<Batch> = None;
    while let Some(b) = stream.next().await {
        let b = b?;
        match &mut out {
            None => out = Some(b),
            Some(acc) => acc.rows.extend(b.rows),
        }
    }
    out.ok_or_else(|| ExecError::Eval("empty stream".into()))
}

/// What a statement produced.
#[derive(Debug, Clone, PartialEq)]
pub enum StatementResult {
    /// A relation (queries).
    Rows(Batch),
    /// A side effect on the store, with a human-readable message.
    Affected {
        /// Rows written or removed.
        rows: u64,
        /// What happened, for the transcript.
        message: String,
    },
    /// The session's final answer.
    Final(Batch),
    /// A setting was recorded; the harness applies it.
    Set {
        /// Setting key.
        key: String,
        /// Setting value.
        value: String,
    },
}

/// Execute one resolved statement.
pub async fn execute_statement(
    stmt: &Statement,
    ctx: ExecContext,
) -> Result<StatementResult, ExecError> {
    match &stmt.kind {
        StatementKind::Query { plan } => Ok(StatementResult::Rows(
            collect(execute(plan, ctx).await?).await?,
        )),
        StatementKind::Final { plan } => Ok(StatementResult::Final(
            collect(execute(plan, ctx).await?).await?,
        )),
        StatementKind::CreateTableAs {
            name,
            plan,
            if_not_exists,
        } => {
            let batch = collect(execute(plan, ctx.clone()).await?).await?;
            let n = batch.len() as u64;
            let created = ctx
                .sink
                .create_table(name, batch.schema.clone(), *if_not_exists)
                .await?;
            if !created {
                return Ok(StatementResult::Affected {
                    rows: 0,
                    message: format!("table {name} already exists; left unchanged"),
                });
            }
            ctx.sink.insert(name, batch).await?;
            Ok(StatementResult::Affected {
                rows: n,
                message: format!("created table {name} with {n} rows"),
            })
        }
        StatementKind::Insert { table, plan } => {
            let batch = collect(execute(plan, ctx.clone()).await?).await?;
            let n = batch.len() as u64;
            ctx.sink.insert(table, batch).await?;
            Ok(StatementResult::Affected {
                rows: n,
                message: format!("inserted {n} rows into {table}"),
            })
        }
        StatementKind::DropTable { name, if_exists } => {
            ctx.sink.drop_table(name, *if_exists).await?;
            Ok(StatementResult::Affected {
                rows: 0,
                message: format!("dropped table {name}"),
            })
        }
        StatementKind::Set { key, value } => Ok(StatementResult::Set {
            key: key.clone(),
            value: value.clone(),
        }),
        StatementKind::Explain { .. } => Err(ExecError::Eval(
            "EXPLAIN is handled by the planner (arrives in M2)".into(),
        )),
        StatementKind::Call { tool, args, input } => {
            // A side effect: once per input row, in input order, never
            // deduplicated, never concurrent.
            let env = ops::Env::default();
            let rows: Vec<Row> = match input {
                Some(p) => ops::eval_plan(p, &env, &ctx).await?,
                None => vec![vec![]],
            };
            let mut out: Vec<Row> = vec![];
            let mut schema: Option<Arc<Schema>> = None;
            for r in &rows {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(eval::eval_expr(a, r, &env, &ctx).await?);
                }
                let batch = ctx.sink.table_call(tool, &vals).await?;
                if schema.is_none() {
                    schema = Some(batch.schema.clone());
                }
                out.extend(batch.rows);
            }
            let schema = schema.unwrap_or_else(|| Arc::new(Schema::empty()));
            let batch = Batch::try_new(schema, out)?;
            Ok(StatementResult::Rows(batch))
        }
        StatementKind::CreateFunction { .. } => Err(ExecError::Eval(
            "CREATE FUNCTION is handled by the harness".into(),
        )),
        StatementKind::CreateAgent { .. } => Err(ExecError::Eval(
            "CREATE AGENT is handled by the harness".into(),
        )),
        StatementKind::Calibrate { .. } => Err(ExecError::Eval(
            "CALIBRATE is handled by the harness".into(),
        )),
    }
}
