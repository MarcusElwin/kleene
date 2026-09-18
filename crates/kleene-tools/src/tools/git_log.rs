//! `git_log([n BIGINT])`: recent commits.

use crate::args::{check_arity, opt_int};
use crate::tools::git::git;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// Commits returned when `n` is not given.
pub const DEFAULT_LIMIT: i64 = 20;

/// `git_log([n BIGINT]) -> (sha TEXT, author TEXT, date TEXT, message TEXT)`.
///
/// The `n` most recent commits reachable from `HEAD` (default 20), newest
/// first. `date` is the author date in ISO 8601; `message` is the full
/// commit message. A repository with no commits yields no rows.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitLog;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for GitLog {
    fn name(&self) -> &str {
        "git_log"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[], &[DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("sha", DataType::Text),
                    Field::not_null("author", DataType::Text),
                    Field::not_null("date", DataType::Text),
                    Field::not_null("message", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "the n most recent commits (default 20): sha, author, date, message"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let n = opt_int(args, 0, "n")?.unwrap_or(DEFAULT_LIMIT);
        if n <= 0 {
            return Err(ToolError::Args(format!("n must be positive, got {n}")));
        }
        let limit = n.to_string();
        let out = match git(
            ctx,
            &["log", "-n", &limit, "--format=%H%x1f%an%x1f%aI%x1f%B%x1e"],
        )
        .await
        {
            Ok(out) => out,
            Err(ToolError::Other(msg)) if msg.contains("does not have any commits") => {
                String::new()
            }
            Err(e) => return Err(e),
        };
        let rows = out
            .split('\x1e')
            .map(str::trim)
            .filter(|rec| !rec.is_empty())
            .filter_map(|rec| {
                let mut parts = rec.splitn(4, '\x1f');
                Some(vec![
                    Value::from(parts.next()?),
                    Value::from(parts.next()?),
                    Value::from(parts.next()?),
                    Value::from(parts.next()?.trim_end()),
                ])
            })
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

    #[tokio::test]
    async fn lists_commits_newest_first() {
        let Some((dir, ctx)) = test_repo() else {
            return;
        };
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        commit_all(dir.path(), "add b");

        let b = GitLog.call(&[], &ctx).await.unwrap();
        assert_eq!(b.len(), 2);
        let first = &b.rows[0];
        assert_eq!(first[0].render().len(), 40);
        assert_eq!(first[1].render(), "Second Author");
        assert!(first[2].render().contains('T'), "{}", first[2]);
        assert_eq!(first[3].render(), "add b");
        let second = &b.rows[1];
        assert_eq!(second[1].render(), "Test Author");
        assert_eq!(second[3].render(), "initial commit\n\nwith a body");

        let b = GitLog.call(&[Value::Int(1)], &ctx).await.unwrap();
        assert_eq!(b.len(), 1);
        assert!(GitLog.call(&[Value::Int(0)], &ctx).await.is_err());
    }

    #[tokio::test]
    async fn empty_repository_has_no_rows_and_no_repo_is_an_error() {
        let Some(_) = test_repo() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let err = GitLog.call(&[], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Other(_)), "{err}");
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let b = GitLog.call(&[], &ctx).await.unwrap();
        assert!(b.is_empty());
    }
}
