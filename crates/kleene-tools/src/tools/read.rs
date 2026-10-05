//! `read(path TEXT)`: a whole file as one row, documents converted to text.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access};
use crate::tools::convert::{document_text, DOCUMENT_EXTENSIONS};
use crate::tools::lines::read_recorded;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// `read(path TEXT) -> (text TEXT)`, exactly one row. Records the read for
/// the staleness check in `patch`. `.docx`, `.xlsx`, `.pptx`, `.pdf` and
/// `.eml` come back as text (see `convert`); everything else as is.
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
        "the whole text of a workspace file as one row (.docx, .xlsx, .pptx, .pdf and .eml converted to text)"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if ext.is_some_and(|e| DOCUMENT_EXTENSIONS.contains(&e.as_str())) {
            let resolved = resolve(ctx, path, Access::Read)?;
            let meta = std::fs::metadata(&resolved.abs)?;
            if meta.is_dir() {
                return Err(ToolError::Args(format!("{path} is a directory")));
            }
            let contents = document_text(&resolved.abs).await?;
            if let Ok(mtime) = meta.modified() {
                ctx.record_read(resolved.abs.clone(), mtime);
            }
            return Ok(Batch {
                schema: self.schema(),
                rows: vec![vec![Value::Text(contents)]],
            });
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
    async fn documents_are_converted_and_missing_ones_are_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        for name in ["r.PDF", "d.docx", "s.xlsx", "p.pptx", "m.eml"] {
            let err = Read.call(&[Value::from(name)], &ctx).await.unwrap_err();
            assert!(matches!(err, ToolError::Io(_)), "{name}: {err}");
        }
        let docx = dir.path().join("memo.docx");
        crate::tools::convert::write_document(&docx, "# Memo\nBody line.")
            .await
            .unwrap();
        let b = Read.call(&[Value::from("memo.docx")], &ctx).await.unwrap();
        assert_eq!(b.rows[0][0].render(), "Memo\nBody line.\n");
        assert!(ctx.last_read(&docx.canonicalize().unwrap()).is_some());
    }
}
