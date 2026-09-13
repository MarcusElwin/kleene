//! In-memory [`Store`] for tests.

use crate::{Store, StoreError};
use callgebra_core::{Batch, Schema};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tables in a map. Not persistent, not the production store.
#[derive(Default)]
pub struct MemoryStore {
    tables: RwLock<BTreeMap<String, Batch>>,
}

impl MemoryStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl Store for MemoryStore {
    async fn create_table(&self, name: &str, schema: Arc<Schema>) -> Result<(), StoreError> {
        let mut t = self.tables.write().await;
        let key = name.to_ascii_lowercase();
        if t.contains_key(&key) {
            return Err(StoreError::AlreadyExists(name.to_string()));
        }
        t.insert(key, Batch::empty(schema));
        Ok(())
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), StoreError> {
        let mut t = self.tables.write().await;
        let existing = t
            .get_mut(&name.to_ascii_lowercase())
            .ok_or_else(|| StoreError::NoSuchTable(name.to_string()))?;
        if existing.schema.len() != batch.schema.len() {
            return Err(StoreError::SchemaMismatch {
                table: name.to_string(),
                detail: format!(
                    "expected {} columns, got {}",
                    existing.schema.len(),
                    batch.schema.len()
                ),
            });
        }
        existing.rows.extend(batch.rows);
        Ok(())
    }

    async fn drop_table(&self, name: &str) -> Result<(), StoreError> {
        self.tables
            .write()
            .await
            .remove(&name.to_ascii_lowercase())
            .map(|_| ())
            .ok_or_else(|| StoreError::NoSuchTable(name.to_string()))
    }

    async fn scan(&self, name: &str) -> Result<Batch, StoreError> {
        self.tables
            .read()
            .await
            .get(&name.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| StoreError::NoSuchTable(name.to_string()))
    }

    async fn schema(&self, name: &str) -> Result<Option<Arc<Schema>>, StoreError> {
        Ok(self
            .tables
            .read()
            .await
            .get(&name.to_ascii_lowercase())
            .map(|b| b.schema.clone()))
    }

    async fn tables(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.tables.read().await.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callgebra_core::{DataType, Field, Value};

    #[tokio::test]
    async fn create_insert_scan_drop() {
        let s = MemoryStore::new();
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int)]));
        s.create_table("T", schema.clone()).await.unwrap();
        s.insert(
            "t",
            Batch::try_new(
                schema.clone(),
                vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            )
            .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(s.scan("t").await.unwrap().len(), 2);
        assert_eq!(s.tables().await.unwrap(), vec!["t".to_string()]);
        s.drop_table("t").await.unwrap();
        assert!(matches!(s.scan("t").await, Err(StoreError::NoSuchTable(_))));
    }
}
