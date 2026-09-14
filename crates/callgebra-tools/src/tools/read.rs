//! `read(path TEXT)`: a whole file as one row.

use crate::args::{check_arity, text};
use crate::tools::lines::read_recorded;
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Extensions of binary document formats whose conversion (via pandoc)
/// arrives in M7; `read` refuses them for now instead of returning bytes.
const BINARY_DOCUMENTS: &[&str] = &["docx", "pdf", "xlsx", "pptx"];

/// `read(path TEXT) -> (text TEXT)`, exactly one row. Records the read for
/// the staleness check in `patch`. Plain text only in M2: `.docx`, `.pdf`,
/// `.xlsx` and `.pptx` return an error until conversion lands in M7.
#[derive(Debug, Default, Clone, Copy)]
pub struct Read;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "read"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| Arc::new(Schema::new(vec![Field::not_null("text", DataType::Text)])))
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "the whole text of a workspace file as one row"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if ext.is_some_and(|e| BINARY_DOCUMENTS.contains(&e.as_str())) {
            return Err(ToolError::Other(
                "binary document; conversion arrives in M7".into(),
            ));
        }
        let (_, contents) = read_recorded(ctx, path)?;
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![Value::Text(contents)]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_one_row_and_records_the_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\nworld\n").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Read.call(&[Value::from("a.txt")], &ctx).await.unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b.rows[0][0].render(), "hello\nworld\n");
        let abs = dir.path().canonicalize().unwrap().join("a.txt");
        assert!(ctx.last_read(&abs).is_some());
    }

    #[tokio::test]
    async fn binary_documents_are_refused_before_any_io() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        for name in ["r.PDF", "d.docx", "s.xlsx", "p.pptx"] {
            let err = Read.call(&[Value::from(name)], &ctx).await.unwrap_err();
            assert_eq!(err.to_string(), "binary document; conversion arrives in M7");
        }
    }
}
