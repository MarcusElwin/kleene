//! `lines(path TEXT [, from BIGINT, to BIGINT])`: a file, or a range of
//! it, as numbered lines.

use crate::args::{check_arity, opt_int, text};
use crate::paths::{resolve, Access, Resolved};
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// Read a workspace file as text (invalid UTF-8 is replaced) and record its
/// modification time in the context's read registry, so `patch` can later
/// tell whether the model's view is current.
pub(crate) fn read_recorded(
    ctx: &ToolContext,
    path: &str,
) -> Result<(Resolved, String), ToolError> {
    let resolved = resolve(ctx, path, Access::Read)?;
    let meta = std::fs::metadata(&resolved.abs)?;
    if meta.is_dir() {
        return Err(ToolError::Args(format!("{path} is a directory")));
    }
    let bytes = std::fs::read(&resolved.abs)?;
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    if let Ok(mtime) = meta.modified() {
        ctx.record_read(resolved.abs.clone(), mtime);
    }
    Ok((resolved, text))
}

/// `lines(path TEXT [, from BIGINT, to BIGINT]) -> (lineno BIGINT, text TEXT)`,
/// 1-based, without line terminators; `from` and `to` bound the range
/// inclusively (a `to` past the end is fine). Records the read for the
/// staleness check in `patch`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Lines;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Lines {
    fn name(&self) -> &str {
        "lines"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[DataType::Int, DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("lineno", DataType::Int),
                    Field::not_null("text", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "a workspace file, or lines from..to of it, as 1-based numbered lines"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let from = opt_int(args, 1, "from")?.unwrap_or(1).max(1);
        let to = opt_int(args, 2, "to")?.unwrap_or(i64::MAX);
        if to < from {
            return Err(ToolError::Args(format!(
                "to ({to}) is before from ({from})"
            )));
        }
        let (_, contents) = read_recorded(ctx, path)?;
        let rows = contents
            .lines()
            .enumerate()
            .map(|(i, l)| (i as i64 + 1, l))
            .filter(|(n, _)| *n >= from && *n <= to)
            .map(|(n, l)| vec![Value::Int(n), Value::from(l)])
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
    async fn numbers_lines_and_records_the_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\r\nthree").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Lines.call(&[Value::from("a.txt")], &ctx).await.unwrap();
        let got: Vec<(i64, String)> = b
            .rows
            .iter()
            .map(|r| (r[0].as_int().unwrap(), r[1].render()))
            .collect();
        assert_eq!(
            got,
            vec![(1, "one".into()), (2, "two".into()), (3, "three".into())]
        );
        let abs = dir.path().canonicalize().unwrap().join("a.txt");
        assert!(ctx.last_read(&abs).is_some());
    }

    #[tokio::test]
    async fn a_range_bounds_the_lines_inclusively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Lines
            .call(&[Value::from("a.txt"), Value::Int(2), Value::Int(4)], &ctx)
            .await
            .unwrap();
        let got: Vec<i64> = b.rows.iter().map(|r| r[0].as_int().unwrap()).collect();
        assert_eq!(got, vec![2, 3, 4]);
        let b = Lines
            .call(&[Value::from("a.txt"), Value::Int(4), Value::Int(99)], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows.len(), 2);
        let err = Lines
            .call(&[Value::from("a.txt"), Value::Int(4), Value::Int(2)], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
    }

    #[tokio::test]
    async fn refuses_directories_and_escapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        assert!(matches!(
            Lines.call(&[Value::from("d")], &ctx).await,
            Err(ToolError::Args(_))
        ));
        assert!(matches!(
            Lines.call(&[Value::from("../x")], &ctx).await,
            Err(ToolError::Denied(_))
        ));
        assert!(matches!(
            Lines.call(&[Value::from("missing")], &ctx).await,
            Err(ToolError::Io(_))
        ));
    }
}
