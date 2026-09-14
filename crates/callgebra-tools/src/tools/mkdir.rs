//! `mkdir(path TEXT)`: create a directory and its parents.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access};
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// The `(path TEXT)` schema shared by `mkdir` and `remove`.
pub(crate) fn path_schema() -> Arc<Schema> {
    static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| Arc::new(Schema::new(vec![Field::not_null("path", DataType::Text)])))
        .clone()
}

/// `mkdir(path TEXT) -> (path TEXT)`. Creates missing parents; an existing
/// directory is not an error. The path must be under a writable root.
#[derive(Debug, Default, Clone, Copy)]
pub struct Mkdir;

#[async_trait::async_trait]
impl Tool for Mkdir {
    fn name(&self) -> &str {
        "mkdir"
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
        "create a directory (and parents) in the workspace"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let resolved = resolve(ctx, path, Access::Write)?;
        std::fs::create_dir_all(&resolved.abs)?;
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
    async fn creates_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Mkdir.call(&[Value::from("a/b/c")], &ctx).await.unwrap();
        assert_eq!(b.rows[0][0].render(), "a/b/c");
        assert!(dir.path().join("a/b/c").is_dir());
        assert!(Mkdir.call(&[Value::from("a/b/c")], &ctx).await.is_ok());
        let err = Mkdir.call(&[Value::from("../x")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)));
    }
}
