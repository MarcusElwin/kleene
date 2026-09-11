//! The embedded store.
//!
//! One DuckDB file per run holds session tables (`CREATE TABLE AS`), the memo
//! of model calls, and the trace. The same tables are mounted back into every
//! session's catalog as `trace_*` relations. [`DuckDbStore`] is the
//! production store; [`MemoryStore`] serves unit tests elsewhere.

#![forbid(unsafe_code)]

#[cfg(feature = "duckdb")]
pub mod duckdb;
pub mod memory;

use callgebra_core::{Batch, Schema};
use std::sync::Arc;
use thiserror::Error;

#[cfg(feature = "duckdb")]
pub use duckdb::{DuckDbStore, DuckDbTraceSink, MemoEntry};
pub use memory::MemoryStore;

/// Store failures.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Table not found.
    #[error("no such table: {0}")]
    NoSuchTable(String),
    /// Table already exists.
    #[error("table already exists: {0}")]
    AlreadyExists(String),
    /// Schema of inserted rows does not match.
    #[error("schema mismatch for {table}: {detail}")]
    SchemaMismatch {
        /// Table.
        table: String,
        /// What differed.
        detail: String,
    },
    /// Database engine error.
    #[error("store error: {0}")]
    Engine(String),
}

/// Session tables plus the memo and trace tables.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// Create an empty table.
    async fn create_table(&self, name: &str, schema: Arc<Schema>) -> Result<(), StoreError>;
    /// Append rows.
    async fn insert(&self, name: &str, batch: Batch) -> Result<(), StoreError>;
    /// Drop a table.
    async fn drop_table(&self, name: &str) -> Result<(), StoreError>;
    /// Read a whole table. M1 replaces this with a streaming scan for large tables.
    async fn scan(&self, name: &str) -> Result<Batch, StoreError>;
    /// Schema of a table, if it exists.
    async fn schema(&self, name: &str) -> Result<Option<Arc<Schema>>, StoreError>;
    /// All table names.
    async fn tables(&self) -> Result<Vec<String>, StoreError>;
}
