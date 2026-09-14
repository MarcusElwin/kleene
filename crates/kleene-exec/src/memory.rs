//! In-memory [`CallSink`] for tests and the REPL without a store.

use crate::{BatchStream, CallSink, ExecError};
use kleene_core::{Batch, Row, Schema, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A stored table: its schema and rows.
type Table = (Arc<Schema>, Vec<Row>);

/// Tables in a map; no call functions.
#[derive(Default)]
pub struct MemorySink {
    tables: Mutex<HashMap<String, Table>>,
}

impl MemorySink {
    /// Empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a table with rows (replacing any existing one).
    pub async fn load(&self, name: &str, schema: Arc<Schema>, rows: Vec<Row>) {
        self.tables
            .lock()
            .await
            .insert(name.to_ascii_lowercase(), (schema, rows));
    }

    /// Rows of a table, if present.
    pub async fn rows(&self, name: &str) -> Option<Vec<Row>> {
        self.tables
            .lock()
            .await
            .get(&name.to_ascii_lowercase())
            .map(|(_, r)| r.clone())
    }
}

#[async_trait::async_trait]
impl CallSink for MemorySink {
    async fn scalar_call(&self, name: &str, _args: &[Value]) -> Result<Value, ExecError> {
        Err(ExecError::Call(format!(
            "no implementation for scalar function {name}"
        )))
    }

    async fn table_call(&self, name: &str, _args: &[Value]) -> Result<Batch, ExecError> {
        Err(ExecError::Call(format!(
            "no implementation for table function {name}"
        )))
    }

    async fn scan(&self, table: &str) -> Result<BatchStream, ExecError> {
        let (schema, rows) = self
            .tables
            .lock()
            .await
            .get(&table.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| ExecError::NoSuchTable(table.to_string()))?;
        Ok(crate::chunk(schema, rows, 1024))
    }

    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        let mut t = self.tables.lock().await;
        let key = name.to_ascii_lowercase();
        if t.contains_key(&key) {
            return if if_not_exists {
                Ok(false)
            } else {
                Err(ExecError::TableExists(name.to_string()))
            };
        }
        t.insert(key, (schema, vec![]));
        Ok(true)
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), ExecError> {
        let mut t = self.tables.lock().await;
        let (schema, rows) = t
            .get_mut(&name.to_ascii_lowercase())
            .ok_or_else(|| ExecError::NoSuchTable(name.to_string()))?;
        if schema.len() != batch.schema.len() {
            return Err(ExecError::Eval(format!(
                "{name} has {} columns, batch has {}",
                schema.len(),
                batch.schema.len()
            )));
        }
        rows.extend(batch.rows);
        Ok(())
    }

    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError> {
        let removed = self
            .tables
            .lock()
            .await
            .remove(&name.to_ascii_lowercase())
            .is_some();
        if removed || if_exists {
            Ok(())
        } else {
            Err(ExecError::NoSuchTable(name.to_string()))
        }
    }
}
