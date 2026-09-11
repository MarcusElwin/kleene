//! `git_blame(path TEXT)`: who last touched each line.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access};
use crate::tools::git::git;
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// Parse `git blame --line-porcelain` output into
/// `(lineno, sha, author, text)` rows.
pub(crate) fn parse_porcelain(out: &str) -> Vec<(i64, String, String, String)> {
    let mut rows = Vec::new();
    let mut sha = String::new();
    let mut lineno = 0i64;
    let mut author = String::new();
    for line in out.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            rows.push((lineno, sha.clone(), author.clone(), content.to_string()));
            continue;
        }
        let mut words = line.split(' ');
        match words.next() {
            Some(w) if w.len() == 40 && w.bytes().all(|b| b.is_ascii_hexdigit()) => {
                sha = w.to_string();
                lineno = words.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            }
            Some("author") => author = words.collect::<Vec<_>>().join(" "),
            _ => {}
        }
    }
    rows
}

/// `git_blame(path TEXT) -> (lineno BIGINT, sha TEXT, author TEXT, text TEXT)`.
///
/// One row per line of the tracked file as it is in the working tree; the
/// sha is all zeros for uncommitted lines.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitBlame;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for GitBlame {
    fn name(&self) -> &str {
        "git_blame"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("lineno", DataType::Int),
                    Field::not_null("sha", DataType::Text),
                    Field::not_null("author", DataType::Text),
                    Field::not_null("text", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "per-line last commit and author of a tracked file"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let path = text(args, 0, "path")?;
        let resolved = resolve(ctx, path, Access::Read)?;
        let out = git(ctx, &["blame", "--line-porcelain", "--", &resolved.rel]).await?;
        let rows = parse_porcelain(&out)
            .into_iter()
            .map(|(lineno, sha, author, text)| {
                vec![
                    Value::Int(lineno),
                    Value::Text(sha),
                    Value::Text(author),
                    Value::Text(text),
                ]
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

    #[test]
    fn parses_line_porcelain() {
        let out = "0123456789012345678901234567890123456789 1 1 2\nauthor Ada Lovelace\n\
                   author-mail <a@x>\n\tfirst\n\
                   0123456789012345678901234567890123456789 2 2\nauthor Ada Lovelace\n\tsecond\n";
        let rows = parse_porcelain(out);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[0].2, "Ada Lovelace");
        assert_eq!(rows[0].3, "first");
        assert_eq!(rows[1].0, 2);
        assert_eq!(rows[1].3, "second");
    }

    #[tokio::test]
    async fn blames_each_line() {
        let Some((dir, ctx)) = test_repo() else {
            return;
        };
        std::fs::write(dir.path().join("a.txt"), "first line\nsecond line\nthird\n").unwrap();
        commit_all(dir.path(), "add third");
        let b = GitBlame.call(&[Value::from("a.txt")], &ctx).await.unwrap();
        assert_eq!(b.len(), 3);
        let rows: Vec<(i64, String, String)> = b
            .rows
            .iter()
            .map(|r| (r[0].as_int().unwrap(), r[2].render(), r[3].render()))
            .collect();
        assert_eq!(rows[0], (1, "Test Author".into(), "first line".into()));
        assert_eq!(rows[2], (3, "Second Author".into(), "third".into()));
        assert_ne!(b.rows[0][1], b.rows[2][1]);
        assert_eq!(b.rows[0][1].render().len(), 40);

        assert!(matches!(
            GitBlame.call(&[Value::from("../x")], &ctx).await,
            Err(ToolError::Denied(_))
        ));
        assert!(GitBlame
            .call(&[Value::from("untracked")], &ctx)
            .await
            .is_err());
    }
}
