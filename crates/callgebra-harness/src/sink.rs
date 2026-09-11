//! A [`CallSink`] over the DuckDB store, for the REPL and the harness.
//!
//! Scans, `CREATE TABLE AS`, `INSERT` and `DROP TABLE` go to the store; call
//! functions and table functions are refused until M2 wires providers and
//! tools in. The sink also keeps the session [`Catalog`] in step with the
//! store, so a table created by one statement is visible to the next.

use callgebra_core::{
    Batch, Catalog, Schema, TableDef, TableSource, Value, Volatility,
};
use callgebra_exec::{BatchStream, CallSink, ExecError};
use callgebra_store::{DuckDbStore, Store, StoreError};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Store-backed sink with a live catalog.
pub struct StoreSink {
    store: DuckDbStore,
    catalog: Arc<RwLock<Catalog>>,
}

fn store_err(e: StoreError) -> ExecError {
    match e {
        StoreError::NoSuchTable(n) => ExecError::NoSuchTable(n),
        StoreError::AlreadyExists(n) => ExecError::TableExists(n),
        other => ExecError::Eval(other.to_string()),
    }
}

impl StoreSink {
    /// Wrap a store; the catalog starts from `base` (standard functions plus
    /// any virtual tables) and picks up every stored table.
    pub async fn new(store: DuckDbStore, base: Catalog) -> Result<Self, StoreError> {
        let sink = Self {
            store,
            catalog: Arc::new(RwLock::new(base)),
        };
        sink.refresh().await?;
        Ok(sink)
    }

    /// The catalog handle, shared with the planner.
    pub fn catalog(&self) -> Arc<RwLock<Catalog>> {
        self.catalog.clone()
    }

    /// The underlying store.
    pub fn store(&self) -> &DuckDbStore {
        &self.store
    }

    /// Re-read stored tables into the catalog.
    pub async fn refresh(&self) -> Result<(), StoreError> {
        let names = self.store.tables().await?;
        let mut defs = Vec::with_capacity(names.len());
        for n in names {
            if let Some(schema) = self.store.schema(&n).await? {
                defs.push(TableDef {
                    name: n,
                    schema: (*schema).clone(),
                    source: TableSource::Stored,
                    volatility: Volatility::Stable,
                    description: "session table".into(),
                });
            }
        }
        let mut cat = self.catalog.write().await;
        let stored: Vec<String> = cat
            .tables()
            .filter(|t| t.source == TableSource::Stored)
            .map(|t| t.name.clone())
            .collect();
        for s in stored {
            cat.remove_table(&s);
        }
        for d in defs {
            cat.add_table(d);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl CallSink for StoreSink {
    async fn scalar_call(&self, name: &str, _args: &[Value]) -> Result<Value, ExecError> {
        Err(ExecError::Call(format!(
            "function {name} needs a model or tool provider (arrives in M2)"
        )))
    }

    async fn table_call(&self, name: &str, _args: &[Value]) -> Result<Batch, ExecError> {
        Err(ExecError::Call(format!(
            "table function {name} needs a tool provider (arrives in M2)"
        )))
    }

    async fn scan(&self, table: &str) -> Result<BatchStream, ExecError> {
        let batch = self.store.scan(table).await.map_err(store_err)?;
        Ok(callgebra_exec::chunk(batch.schema.clone(), batch.rows, 1024))
    }

    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        match self.store.create_table(name, schema).await {
            Ok(()) => {
                self.refresh().await.map_err(store_err)?;
                Ok(true)
            }
            Err(StoreError::AlreadyExists(_)) if if_not_exists => Ok(false),
            Err(e) => Err(store_err(e)),
        }
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), ExecError> {
        self.store.insert(name, batch).await.map_err(store_err)
    }

    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError> {
        match self.store.drop_table(name).await {
            Ok(()) => {
                self.refresh().await.map_err(store_err)?;
                Ok(())
            }
            Err(StoreError::NoSuchTable(_)) if if_exists => Ok(()),
            Err(e) => Err(store_err(e)),
        }
    }
}
