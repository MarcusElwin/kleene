//! A [`CallSink`] over the DuckDB store, for the REPL and the harness.
//!
//! Scans, `CREATE TABLE AS`, `INSERT` and `DROP TABLE` go to the store; call
//! functions and table functions are refused until M2 wires providers and
//! tools in. The sink also keeps the session [`Catalog`] in step with the
//! store, so a table created by one statement is visible to the next.

use kleene_core::{Batch, Catalog, Schema, TableDef, TableSource, Value, Volatility};
use kleene_exec::{BatchStream, CallSink, ExecError};
use kleene_store::{DuckDbStore, Store, StoreError};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Store-backed sink with a live catalog.
pub struct StoreSink {
    store: DuckDbStore,
    catalog: Arc<RwLock<Catalog>>,
    /// Table-name prefix for a child session's private tables (`None` for the
    /// root). Physical names are `cgs_{ns}__{name}`; the root never lists them.
    namespace: Option<String>,
}

/// Prefix marking a session namespace in physical table names.
const NAMESPACE_MARK: &str = "cgs_";

/// The physical prefix of a namespace.
fn prefix(ns: &str) -> String {
    format!("{NAMESPACE_MARK}{ns}__")
}

/// Whether a physical table name belongs to some session namespace.
pub fn is_namespaced(physical: &str) -> bool {
    physical.starts_with(NAMESPACE_MARK) && physical.contains("__")
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
        Self::with_namespace(store, base, None).await
    }

    /// A sink whose tables live under a namespace, so concurrent child
    /// sessions can each have their own `ctx` and scratch tables in one store.
    pub async fn with_namespace(
        store: DuckDbStore,
        base: Catalog,
        namespace: Option<String>,
    ) -> Result<Self, StoreError> {
        let sink = Self {
            store,
            catalog: Arc::new(RwLock::new(base)),
            namespace,
        };
        sink.refresh().await?;
        Ok(sink)
    }

    /// The namespace, if any.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Physical table name for a logical one.
    pub fn physical(&self, name: &str) -> String {
        match &self.namespace {
            Some(ns) => format!("{}{}", prefix(ns), name.to_ascii_lowercase()),
            None => name.to_string(),
        }
    }

    /// Logical name for a physical one, if it belongs to this sink.
    fn logical(&self, physical: &str) -> Option<String> {
        match &self.namespace {
            Some(ns) => physical.strip_prefix(&prefix(ns)).map(str::to_string),
            None if is_namespaced(physical) => None,
            None => Some(physical.to_string()),
        }
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
        for physical in names {
            let Some(n) = self.logical(&physical) else {
                continue;
            };
            if let Some(schema) = self.store.schema(&physical).await? {
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
        let batch = self
            .store
            .scan(&self.physical(table))
            .await
            .map_err(|e| match e {
                StoreError::NoSuchTable(_) => StoreError::NoSuchTable(table.to_string()),
                other => other,
            })
            .map_err(store_err)?;
        Ok(kleene_exec::chunk(batch.schema.clone(), batch.rows, 1024))
    }

    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        match self.store.create_table(&self.physical(name), schema).await {
            Ok(()) => {
                self.refresh().await.map_err(store_err)?;
                Ok(true)
            }
            Err(StoreError::AlreadyExists(_)) if if_not_exists => Ok(false),
            Err(e) => Err(store_err(e)),
        }
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), ExecError> {
        self.store
            .insert(&self.physical(name), batch)
            .await
            .map_err(store_err)
    }

    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError> {
        match self.store.drop_table(&self.physical(name)).await {
            Ok(()) => {
                self.refresh().await.map_err(store_err)?;
                Ok(())
            }
            Err(StoreError::NoSuchTable(_)) if if_exists => Ok(()),
            Err(e) => Err(store_err(e)),
        }
    }
}
