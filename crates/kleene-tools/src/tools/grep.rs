//! `grep(pattern TEXT [, glob TEXT])`: regex search across the workspace.

use crate::args::{check_arity, opt_text, text};
use crate::tools::files::walk;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use regex::Regex;
use std::sync::{Arc, OnceLock};

/// Most matches one `grep` call returns before it appends the truncation row.
pub const MAX_MATCHES: usize = 10_000;
/// Files larger than this are skipped.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Bytes inspected for a NUL to decide a file is binary.
const SNIFF_BYTES: usize = 8 * 1024;
/// Text of the trailing row when the match cap is hit.
pub const TRUNCATED: &str = "... truncated";

/// `grep(pattern TEXT [, glob TEXT]) -> (path TEXT, lineno BIGINT, text TEXT)`.
///
/// `pattern` is a Rust regex, `glob` defaults to `**/*`. Binary files (a
/// NUL in the first 8 KiB) and files over 4 MiB are skipped. Past
/// [`MAX_MATCHES`] matches the result ends with one row whose `lineno` is 0
/// and `text` is `... truncated`, naming the file where the search stopped.
#[derive(Debug, Default, Clone, Copy)]
pub struct Grep;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[DataType::Text])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("path", DataType::Text),
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
        "lines matching a regex in workspace files, optionally limited to a glob"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let pattern = text(args, 0, "pattern")?;
        let glob = opt_text(args, 1, "glob")?.unwrap_or("**/*");
        let re = Regex::new(pattern)
            .map_err(|e| ToolError::Args(format!("bad pattern {pattern:?}: {e}")))?;
        let mut rows = Vec::new();
        'files: for entry in walk(ctx, glob)? {
            if !entry.meta.is_file() || entry.meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let bytes = match std::fs::read(&entry.abs) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
                continue;
            }
            let contents = String::from_utf8_lossy(&bytes);
            for (i, line) in contents.lines().enumerate() {
                if !re.is_match(line) {
                    continue;
                }
                if rows.len() >= MAX_MATCHES {
                    rows.push(vec![
                        Value::Text(entry.rel.clone()),
                        Value::Int(0),
                        Value::from(TRUNCATED),
                    ]);
                    break 'files;
                }
                rows.push(vec![
                    Value::Text(entry.rel.clone()),
                    Value::Int(i as i64 + 1),
                    Value::from(line),
                ]);
            }
        }
        Ok(Batch {
            schema: self.schema(),
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::write(
            p.join("src/a.rs"),
            "fn foo() {}\nfn bar() {}\n// foo again\n",
        )
        .unwrap();
        std::fs::write(p.join("notes.md"), "foo in markdown\n").unwrap();
        std::fs::write(p.join("blob.bin"), b"foo\0foo").unwrap();
        let ctx = ToolContext::new(p.to_path_buf());
        (dir, ctx)
    }

    fn triples(b: &Batch) -> Vec<(String, i64, String)> {
        b.rows
            .iter()
            .map(|r| (r[0].render(), r[1].as_int().unwrap(), r[2].render()))
            .collect()
    }

    #[tokio::test]
    async fn finds_matches_across_files_and_skips_binaries() {
        let (_d, ctx) = ws();
        let b = Grep.call(&[Value::from("foo")], &ctx).await.unwrap();
        assert_eq!(
            triples(&b),
            vec![
                ("notes.md".into(), 1, "foo in markdown".into()),
                ("src/a.rs".into(), 1, "fn foo() {}".into()),
                ("src/a.rs".into(), 3, "// foo again".into()),
            ]
        );
        let b = Grep
            .call(&[Value::from("^fn"), Value::from("**/*.rs")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.len(), 2);
    }

    #[tokio::test]
    async fn bad_regex_is_an_argument_error() {
        let (_d, ctx) = ws();
        let err = Grep.call(&[Value::from("(")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
    }

    #[tokio::test]
    async fn caps_matches_with_a_truncation_row() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x\n".repeat(MAX_MATCHES + 5);
        std::fs::write(dir.path().join("big.txt"), big).unwrap();
        std::fs::write(dir.path().join("zzz.txt"), "x\n").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Grep.call(&[Value::from("x")], &ctx).await.unwrap();
        assert_eq!(b.len(), MAX_MATCHES + 1);
        let last = b.rows.last().unwrap();
        assert_eq!(last[0].render(), "big.txt");
        assert_eq!(last[1], Value::Int(0));
        assert_eq!(last[2].render(), TRUNCATED);
    }

    #[tokio::test]
    async fn skips_files_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(dir.path().join("huge.txt")).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        std::fs::write(dir.path().join("small.txt"), "needle\n").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = Grep.call(&[Value::from("needle|^$")], &ctx).await.unwrap();
        assert_eq!(triples(&b), vec![("small.txt".into(), 1, "needle".into())]);
    }
}
