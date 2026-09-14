//! `files(glob TEXT)`: the workspace listing.

use crate::args::{check_arity, rfc3339, text};
use crate::paths::{relative, workspace_root};
use crate::{Signature, Tool, ToolContext, ToolError};
use globset::{GlobBuilder, GlobMatcher};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::fs::Metadata;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use walkdir::WalkDir;

/// Most entries one `files` call may return.
pub const MAX_ENTRIES: usize = 10_000;

/// Directory names never descended into.
const SKIPPED_DIRS: &[&str] = &[".git", "target"];

/// One workspace entry matched by a glob.
pub(crate) struct Entry {
    /// Path relative to the workspace, `/`-separated.
    pub rel: String,
    /// Absolute path.
    pub abs: PathBuf,
    /// Metadata (not following symlinks).
    pub meta: Metadata,
}

/// Compile a glob for matching workspace-relative paths. `*` never crosses a
/// `/`; `**` matches any number of directories. A leading `./` is ignored.
pub(crate) fn matcher(glob: &str) -> Result<GlobMatcher, ToolError> {
    let pattern = glob.strip_prefix("./").unwrap_or(glob);
    if pattern.is_empty() {
        return Err(ToolError::Args("glob must not be empty".into()));
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| ToolError::Args(format!("bad glob {glob:?}: {e}")))
}

/// Every entry under the workspace matching `glob`, sorted by path,
/// skipping `.git` and `target` directories. Errors with
/// [`ToolError::Other`] past [`MAX_ENTRIES`].
pub(crate) fn walk(ctx: &ToolContext, glob: &str) -> Result<Vec<Entry>, ToolError> {
    let root = workspace_root(ctx)?;
    let matcher = matcher(glob)?;
    let mut out = Vec::new();
    let walker = WalkDir::new(&root)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIPPED_DIRS.contains(&n)))
        });
    for entry in walker {
        let entry = entry.map_err(|e| ToolError::Other(format!("files({glob:?}): {e}")))?;
        let rel = relative(&root, entry.path());
        if !matcher.is_match(&rel) {
            continue;
        }
        let meta = entry.metadata().map_err(std::io::Error::from)?;
        if out.len() >= MAX_ENTRIES {
            return Err(ToolError::Other(format!(
                "files({glob:?}) matches more than {MAX_ENTRIES} entries; narrow the glob"
            )));
        }
        out.push(Entry {
            rel,
            abs: entry.into_path(),
            meta,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// `files(glob TEXT) -> (path TEXT, size BIGINT, mtime TEXT, kind TEXT)`.
///
/// Paths are relative to the workspace and sorted; `kind` is `file` or
/// `dir`; `mtime` is RFC 3339 UTC. `.git` and `target` are never listed.
#[derive(Debug, Default, Clone, Copy)]
pub struct Files;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Files {
    fn name(&self) -> &str {
        "files"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("path", DataType::Text),
                    Field::not_null("size", DataType::Int),
                    Field::not_null("mtime", DataType::Text),
                    Field::not_null("kind", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "workspace entries matching a glob: path, size, mtime, kind ('file' or 'dir')"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let glob = text(args, 0, "glob")?;
        let rows = walk(ctx, glob)?
            .into_iter()
            .map(|e| {
                let mtime = e.meta.modified().map(rfc3339).unwrap_or_default();
                let kind = if e.meta.is_dir() { "dir" } else { "file" };
                vec![
                    Value::Text(e.rel),
                    Value::Int(e.meta.len() as i64),
                    Value::Text(mtime),
                    Value::from(kind),
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

    fn ws() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("src/deep")).unwrap();
        std::fs::create_dir_all(p.join(".git/objects")).unwrap();
        std::fs::create_dir_all(p.join("target/debug")).unwrap();
        std::fs::write(p.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(p.join("src/deep/lib.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(p.join("README.md"), "# hi\n").unwrap();
        std::fs::write(p.join(".git/HEAD"), "ref\n").unwrap();
        std::fs::write(p.join("target/debug/bin"), "\0\0").unwrap();
        let ctx = ToolContext::new(p.to_path_buf());
        (dir, ctx)
    }

    fn col(b: &Batch, i: usize) -> Vec<String> {
        b.column(i).map(Value::render).collect()
    }

    #[tokio::test]
    async fn lists_matching_entries_sorted_and_skips_git_and_target() {
        let (_d, ctx) = ws();
        let b = Files.call(&[Value::from("**")], &ctx).await.unwrap();
        assert_eq!(
            col(&b, 0),
            vec![
                "README.md",
                "src",
                "src/deep",
                "src/deep/lib.rs",
                "src/main.rs"
            ]
        );
        assert_eq!(col(&b, 3), vec!["file", "dir", "dir", "file", "file"]);
        let sizes: Vec<i64> = b.column(1).map(|v| v.as_int().unwrap()).collect();
        assert_eq!(sizes[0], 5);
        for m in col(&b, 2) {
            assert!(m.ends_with('Z') && m.contains('T'), "{m}");
        }
    }

    #[tokio::test]
    async fn star_does_not_cross_directories_but_doublestar_does() {
        let (_d, ctx) = ws();
        let b = Files.call(&[Value::from("*.rs")], &ctx).await.unwrap();
        assert!(b.is_empty());
        let b = Files.call(&[Value::from("**/*.rs")], &ctx).await.unwrap();
        assert_eq!(col(&b, 0), vec!["src/deep/lib.rs", "src/main.rs"]);
        let b = Files.call(&[Value::from("./src/*")], &ctx).await.unwrap();
        assert_eq!(col(&b, 0), vec!["src/deep", "src/main.rs"]);
    }

    #[tokio::test]
    async fn bad_glob_is_an_argument_error() {
        let (_d, ctx) = ws();
        let err = Files.call(&[Value::from("[")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
        let err = Files.call(&[Value::Null], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)));
        let err = Files.call(&[], &ctx).await.unwrap_err();
        assert!(err.to_string().contains("files takes 1 argument"));
    }

    #[tokio::test]
    async fn too_many_entries_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_ENTRIES + 1) {
            std::fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let err = Files.call(&[Value::from("*")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Other(_)), "{err}");
        assert!(err.to_string().contains("narrow the glob"));
    }
}
