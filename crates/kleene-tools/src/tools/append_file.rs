//! `append_file(path TEXT, text TEXT)`: append to a file, creating it.

use crate::args::{check_arity, text};
use crate::tools::write_file::{path_bytes_schema, prepare_write, record_written};
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Schema, Value, Volatility};
use std::io::Write;
use std::sync::Arc;

/// `append_file(path TEXT, text TEXT) -> (path TEXT, bytes BIGINT)`.
///
/// Creates the file and its parents if needed; `bytes` counts what was
/// appended. The path must be under a writable root. Counts as a read for
/// `patch`.
#[derive(Debug, Default, Clone, Copy)]
pub struct AppendFile;

#[async_trait::async_trait]
impl Tool for AppendFile {
    fn name(&self) -> &str {
        "append_file"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text, DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        path_bytes_schema()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "append text to a workspace file, creating it if needed; returns path and bytes appended"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let contents = text(args, 1, "text")?;
        let resolved = prepare_write(ctx, path)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&resolved.abs)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        drop(file);
        record_written(ctx, &resolved)?;
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![
                Value::Text(resolved.rel),
                Value::Int(contents.len() as i64),
            ]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn appends_and_creates() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = AppendFile
            .call(&[Value::from("log/x.txt"), Value::from("one\n")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows[0][0].render(), "log/x.txt");
        assert_eq!(b.rows[0][1], Value::Int(4));
        AppendFile
            .call(&[Value::from("log/x.txt"), Value::from("two\n")], &ctx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("log/x.txt")).unwrap(),
            "one\ntwo\n"
        );
    }

    #[tokio::test]
    async fn denies_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let err = AppendFile
            .call(&[Value::from("/etc/hosts"), Value::from("x")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
    }
}
