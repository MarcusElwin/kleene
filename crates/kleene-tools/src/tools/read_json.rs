//! `read_json(path TEXT)`: a json file as `(row BIGINT, record JSON)`.

use crate::args::{check_arity, text};
use crate::tools::lines::read_recorded;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `read_json(path TEXT) -> (row BIGINT, record JSON)`: one record per
/// element, 1-based; see [`crate::tabular`] for what counts as a record.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReadJson;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for ReadJson {
    fn name(&self) -> &str {
        "read_json"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("row", DataType::Int),
                    Field::not_null("record", DataType::Json),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "a JSON file in the workspace (an array, an object holding one array, or one value per line) as one JSON record per element: json_extract_string(record, '$.key') picks a field"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let (_, contents) = read_recorded(ctx, path).map_err(|e| match e {
            ToolError::Io(io) => {
                ToolError::Io(std::io::Error::new(io.kind(), format!("{path}: {io}")))
            }
            other => other,
        })?;
        let records = crate::tabular::json_records(&contents)
            .map_err(|e| ToolError::Args(format!("{path}: {e}")))?;
        let rows = records
            .into_iter()
            .enumerate()
            .map(|(i, r)| vec![Value::Int(i as i64 + 1), Value::Json(r)])
            .collect();
        Ok(Batch {
            schema: self.schema(),
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn elements_are_records() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.json"), "[{\"id\": 1}, {\"id\": 2}]").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = ReadJson.call(&[Value::from("t.json")], &ctx).await.unwrap();
        assert_eq!(b.rows.len(), 2);
        assert_eq!(b.rows[1][1], Value::Json(serde_json::json!({"id": 2})));
    }
}
