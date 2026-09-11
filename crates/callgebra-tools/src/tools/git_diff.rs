//! `git_diff([ref TEXT])`: working-tree changes, one row per file.

use crate::args::{check_arity, opt_text};
use crate::tools::git::git;
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// Split unified diff output into `(path, diff)` pairs, one per
/// `diff --git` header. The path is the post-image (`b/`) side.
pub(crate) fn split_by_file(diff: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in diff.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("diff --git ") {
            let path = header
                .trim_end()
                .rfind(" b/")
                .map(|p| header.trim_end()[p + 3..].to_string())
                .unwrap_or_else(|| header.trim_end().to_string());
            out.push((path, line.to_string()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
        }
    }
    out
}

/// `git_diff([ref TEXT]) -> (path TEXT, diff TEXT)`.
///
/// The working tree compared with `ref` (default `HEAD`), as one unified
/// diff per changed file. `ref` may be any revision or range git accepts
/// (`HEAD~3`, `main..feature`); it must not look like an option.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitDiff;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for GitDiff {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[], &[DataType::Text])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("path", DataType::Text),
                    Field::not_null("diff", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "unified diff of the working tree against a ref (default HEAD), one row per file"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let rev = opt_text(args, 0, "ref")?.unwrap_or("HEAD");
        if rev.starts_with('-') {
            return Err(ToolError::Args(format!("ref {rev:?} looks like an option")));
        }
        let out = git(ctx, &["diff", "--no-color", "--no-ext-diff", rev, "--"]).await?;
        let rows = split_by_file(&out)
            .into_iter()
            .map(|(path, diff)| vec![Value::Text(path), Value::Text(diff)])
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
    use crate::tools::git::{commit_all, test_repo};

    #[test]
    fn splits_headers_into_files() {
        let d = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n\
                 diff --git a/dir/y b/dir/y\n+++ b/dir/y\n";
        let parts = split_by_file(d);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "x.rs");
        assert!(parts[0].1.ends_with("+b\n"));
        assert_eq!(parts[1].0, "dir/y");
        assert!(split_by_file("").is_empty());
    }

    #[tokio::test]
    async fn diffs_the_working_tree_against_a_ref() {
        let Some((dir, ctx)) = test_repo() else {
            return;
        };
        let b = GitDiff.call(&[], &ctx).await.unwrap();
        assert!(b.is_empty());

        std::fs::write(dir.path().join("a.txt"), "first line\nchanged line\n").unwrap();
        let b = GitDiff.call(&[], &ctx).await.unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b.rows[0][0].render(), "a.txt");
        let diff = b.rows[0][1].render();
        assert!(diff.contains("-second line"), "{diff}");
        assert!(diff.contains("+changed line"), "{diff}");

        commit_all(dir.path(), "change a");
        assert!(GitDiff.call(&[], &ctx).await.unwrap().is_empty());
        let b = GitDiff.call(&[Value::from("HEAD~1")], &ctx).await.unwrap();
        assert_eq!(b.len(), 1);
        assert!(GitDiff
            .call(&[Value::from("--output=x")], &ctx)
            .await
            .is_err());
        assert!(GitDiff
            .call(&[Value::from("nosuchref")], &ctx)
            .await
            .is_err());
    }
}
