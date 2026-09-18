//! `patch(path TEXT, old TEXT, new TEXT)`: replace one exact occurrence.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access};
use crate::tools::write_file::record_written;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `patch(path TEXT, old TEXT, new TEXT) -> (path TEXT, replaced BIGINT)`.
///
/// Replaces exactly one occurrence of `old` with `new`; zero or several
/// occurrences is an argument error that says how many were found. The edit
/// is refused with [`ToolError::Denied`] when the file was never read in
/// this session (`read`, `lines`, or a previous write) or has changed on
/// disk since, so the model always edits what it last saw. A successful
/// patch records the new modification time, so a `CALL patch(...) FROM
/// edits` over one file applies every row.
#[derive(Debug, Default, Clone, Copy)]
pub struct Patch;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Patch {
    fn name(&self) -> &str {
        "patch"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text, DataType::Text, DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("path", DataType::Text),
                    Field::not_null("replaced", DataType::Int),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "replace exactly one occurrence of `old` with `new` in a file read earlier and unchanged since"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let old = text(args, 1, "old")?;
        let new = text(args, 2, "new")?;
        if old.is_empty() {
            return Err(ToolError::Args("old must not be empty".into()));
        }
        let resolved = resolve(ctx, path, Access::Write)?;
        let meta = std::fs::metadata(&resolved.abs)?;
        let Some(seen) = ctx.last_read(&resolved.abs) else {
            return Err(ToolError::Denied(format!(
                "{path}: file was never read in this session; read it before patching"
            )));
        };
        if meta.modified().is_ok_and(|now| now > seen) {
            return Err(ToolError::Denied(format!(
                "{path}: file changed since last read; read it again before patching"
            )));
        }
        let contents = std::fs::read_to_string(&resolved.abs)?;
        match contents.matches(old).count() {
            1 => {}
            0 => {
                return Err(ToolError::Args(format!(
                    "old text not found in {path} (0 occurrences)"
                )))
            }
            n => {
                return Err(ToolError::Args(format!(
                    "old text occurs {n} times in {path}; expected exactly one"
                )))
            }
        }
        let patched = contents.replacen(old, new, 1);
        std::fs::write(&resolved.abs, patched)?;
        record_written(ctx, &resolved)?;
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![Value::Text(resolved.rel), Value::Int(1)]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::lines::Lines;
    use crate::Tool;
    use std::time::{Duration, SystemTime};

    fn ws(contents: &str) -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), contents).unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        (dir, ctx)
    }

    async fn read(ctx: &ToolContext) {
        Lines.call(&[Value::from("f.txt")], ctx).await.unwrap();
    }

    fn patch_args(old: &str, new: &str) -> [Value; 3] {
        [Value::from("f.txt"), Value::from(old), Value::from(new)]
    }

    #[tokio::test]
    async fn replaces_a_single_occurrence() {
        let (dir, ctx) = ws("a b c\n");
        read(&ctx).await;
        let b = Patch.call(&patch_args("b", "B"), &ctx).await.unwrap();
        assert_eq!(b.rows[0][0].render(), "f.txt");
        assert_eq!(b.rows[0][1], Value::Int(1));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "a B c\n"
        );
        // A second patch works without re-reading: the patch recorded the mtime.
        Patch.call(&patch_args("c", "C"), &ctx).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "a B C\n"
        );
    }

    #[tokio::test]
    async fn zero_or_many_occurrences_say_how_many() {
        let (_dir, ctx) = ws("x x y\n");
        read(&ctx).await;
        let err = Patch.call(&patch_args("x", "z"), &ctx).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid arguments: old text occurs 2 times in f.txt; expected exactly one"
        );
        let err = Patch.call(&patch_args("q", "z"), &ctx).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid arguments: old text not found in f.txt (0 occurrences)"
        );
        let err = Patch.call(&patch_args("", "z"), &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)));
    }

    #[tokio::test]
    async fn refuses_files_never_read() {
        let (_dir, ctx) = ws("a\n");
        let err = Patch.call(&patch_args("a", "b"), &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        assert!(err.to_string().contains("never read"));
    }

    #[tokio::test]
    async fn refuses_files_changed_since_the_read() {
        let (dir, ctx) = ws("a\n");
        read(&ctx).await;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("f.txt"))
            .unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let err = Patch.call(&patch_args("a", "b"), &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        assert!(err.to_string().contains("file changed since last read"));
        // Reading again clears the staleness.
        read(&ctx).await;
        Patch.call(&patch_args("a", "b"), &ctx).await.unwrap();
    }
}
