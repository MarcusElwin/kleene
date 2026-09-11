//! `remove(path TEXT)`: delete a file or an empty directory.

use crate::args::{check_arity, text};
use crate::paths::{resolve, workspace_root, Access};
use crate::tools::mkdir::path_schema;
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Schema, Value, Volatility};
use std::sync::Arc;

/// `remove(path TEXT) -> (path TEXT)`. Deletes one file, one symlink, or one
/// empty directory under a writable root; a non-empty directory and the
/// workspace root itself are refused. Forgets any recorded read of the file.
#[derive(Debug, Default, Clone, Copy)]
pub struct Remove;

#[async_trait::async_trait]
impl Tool for Remove {
    fn name(&self) -> &str {
        "remove"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        path_schema()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "delete a workspace file or empty directory"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let resolved = resolve(ctx, path, Access::Write)?;
        if resolved.abs == workspace_root(ctx)? {
            return Err(ToolError::Denied(format!(
                "{path}: the workspace root cannot be removed"
            )));
        }
        let meta = std::fs::symlink_metadata(&resolved.abs)?;
        if meta.is_dir() {
            if std::fs::read_dir(&resolved.abs)?.next().is_some() {
                return Err(ToolError::Args(format!("{path}: directory not empty")));
            }
            std::fs::remove_dir(&resolved.abs)?;
        } else {
            std::fs::remove_file(&resolved.abs)?;
        }
        if let Ok(mut reads) = ctx.reads.lock() {
            reads.remove(&resolved.abs);
        }
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![Value::Text(resolved.rel)]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn removes_files_and_empty_dirs_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("full/empty")).unwrap();
        std::fs::write(p.join("full/f.txt"), "x").unwrap();
        let ctx = ToolContext::new(p.to_path_buf());

        let err = Remove.call(&[Value::from("full")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
        assert!(err.to_string().contains("not empty"));

        let b = Remove
            .call(&[Value::from("full/empty")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows[0][0].render(), "full/empty");
        assert!(!p.join("full/empty").exists());

        Remove
            .call(&[Value::from("full/f.txt")], &ctx)
            .await
            .unwrap();
        assert!(!p.join("full/f.txt").exists());
        Remove.call(&[Value::from("full")], &ctx).await.unwrap();
        assert!(!p.join("full").exists());

        let err = Remove
            .call(&[Value::from("missing")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Io(_)), "{err}");
        let err = Remove.call(&[Value::from(".")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        let err = Remove.call(&[Value::from("../x")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
    }
}
