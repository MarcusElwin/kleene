//! `write_file(path TEXT, text TEXT)`: create or replace a file.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access, Resolved};
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// The `(path TEXT, bytes BIGINT)` schema shared by `write_file` and
/// `append_file`.
pub(crate) fn path_bytes_schema() -> Arc<Schema> {
    static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| {
            Arc::new(Schema::new(vec![
                Field::not_null("path", DataType::Text),
                Field::not_null("bytes", DataType::Int),
            ]))
        })
        .clone()
}

/// Resolve a path for writing and create its parent directories.
pub(crate) fn prepare_write(ctx: &ToolContext, path: &str) -> Result<Resolved, ToolError> {
    let resolved = resolve(ctx, path, Access::Write)?;
    if let Some(parent) = resolved.abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(resolved)
}

/// After a write the model authored, treat the file as read: record its new
/// modification time so a following `patch` is not refused as stale.
pub(crate) fn record_written(ctx: &ToolContext, resolved: &Resolved) -> Result<(), ToolError> {
    let meta = std::fs::metadata(&resolved.abs)?;
    if let Ok(mtime) = meta.modified() {
        ctx.record_read(resolved.abs.clone(), mtime);
    }
    Ok(())
}

/// `write_file(path TEXT, text TEXT) -> (path TEXT, bytes BIGINT)`.
///
/// Creates parent directories; replaces any existing content. The path must
/// be under a writable root. The write counts as a read for `patch`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
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
        "create or replace a workspace file with the given text; returns path and bytes written"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let contents = text(args, 1, "text")?;
        let resolved = prepare_write(ctx, path)?;
        std::fs::write(&resolved.abs, contents)?;
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
    use std::path::PathBuf;

    #[tokio::test]
    async fn writes_creating_parents_and_reports_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = WriteFile
            .call(&[Value::from("a/b/c.txt"), Value::from("héllo")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows[0][0].render(), "a/b/c.txt");
        assert_eq!(b.rows[0][1], Value::Int(6));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "héllo"
        );
        let abs = dir.path().canonicalize().unwrap().join("a/b/c.txt");
        assert!(ctx.last_read(&abs).is_some());
        // Replaces.
        WriteFile
            .call(&[Value::from("a/b/c.txt"), Value::from("x")], &ctx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "x"
        );
    }

    #[tokio::test]
    async fn respects_the_writable_list_and_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ToolContext::new(dir.path().to_path_buf());
        ctx.writable = vec![PathBuf::from("out")];
        assert!(WriteFile
            .call(&[Value::from("out/x"), Value::from("1")], &ctx)
            .await
            .is_ok());
        let err = WriteFile
            .call(&[Value::from("elsewhere/x"), Value::from("1")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        let err = WriteFile
            .call(&[Value::from("../x"), Value::from("1")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        assert!(!dir.path().join("elsewhere").exists());
    }
}
