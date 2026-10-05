//! `patch(path TEXT, old TEXT, new TEXT)`: replace one occurrence, exactly
//! or, failing that, modulo whitespace and typographic quotes.

use crate::args::{check_arity, text};
use crate::paths::{resolve, Access};
use crate::tools::write_file::record_written;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `patch(path TEXT, old TEXT, new TEXT) -> (path TEXT, replaced BIGINT, diff TEXT)`.
///
/// Replaces exactly one occurrence of `old` with `new`. The match is exact
/// first; when `old` occurs nowhere verbatim, the file is matched line by
/// line with trailing whitespace ignored and typographic quotes and dashes
/// read as their ASCII forms, so a model that re-typed a line from memory
/// still lands the edit. Zero or several candidates is an argument error
/// that says how many were found. The edit is refused with
/// [`ToolError::Denied`] when the file was never read in this session
/// (`read`, `lines`, or a previous write) or has changed on disk since, so
/// the model always edits what it last saw. A successful patch records the
/// new modification time, so a `CALL patch(...) FROM edits` over one file
/// applies every row, and returns a unified diff of what changed.
#[derive(Debug, Default, Clone, Copy)]
pub struct Patch;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

/// Where `old` sits in `contents`, as a byte range.
fn locate(contents: &str, old: &str) -> Result<std::ops::Range<usize>, ToolError> {
    match contents
        .match_indices(old)
        .map(|(i, _)| i)
        .collect::<Vec<_>>()[..]
    {
        [i] => return Ok(i..i + old.len()),
        [] => {}
        ref many => {
            return Err(ToolError::Args(format!(
                "old text occurs {} times; expected exactly one",
                many.len()
            )))
        }
    }
    // Fuzzy: whole lines, normalised.
    let old_lines: Vec<String> = old.lines().map(normalise).collect();
    if old_lines.is_empty() || old_lines.iter().all(|l| l.is_empty()) {
        return Err(ToolError::Args("old text not found (0 occurrences)".into()));
    }
    let mut starts = vec![0usize];
    for (i, b) in contents.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let lines: Vec<&str> = contents.lines().collect();
    let norm: Vec<String> = lines.iter().map(|l| normalise(l)).collect();
    let mut hits = vec![];
    if norm.len() >= old_lines.len() {
        for i in 0..=norm.len() - old_lines.len() {
            if norm[i..i + old_lines.len()] == old_lines[..] {
                hits.push(i);
            }
        }
    }
    match hits[..] {
        [i] => {
            let start = starts[i];
            let last = i + old_lines.len() - 1;
            let end = starts[last] + lines[last].len();
            Ok(start..end)
        }
        [] => Err(ToolError::Args(
            "old text not found (0 occurrences, exact or ignoring whitespace and quotes)".into(),
        )),
        ref many => Err(ToolError::Args(format!(
            "old text matches {} places ignoring whitespace and quotes; expected exactly one",
            many.len()
        ))),
    }
}

/// A line with trailing whitespace dropped, leading whitespace collapsed to
/// one space, and typographic quotes and dashes mapped to ASCII.
fn normalise(line: &str) -> String {
    let trimmed = line.trim_end();
    let lead = trimmed.len() - trimmed.trim_start().len();
    let mut out = String::with_capacity(trimmed.len());
    if lead > 0 {
        out.push(' ');
    }
    for c in trimmed.trim_start().chars() {
        out.push(match c {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{2032}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{2033}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{a0}' => ' ',
            other => other,
        });
    }
    out
}

/// A unified diff hunk for replacing `range` of `contents` with `new`, with
/// two lines of context on each side.
pub(crate) fn unified_diff(contents: &str, range: &std::ops::Range<usize>, new: &str) -> String {
    let before = &contents[..range.start];
    let after = &contents[range.end..];
    // The replaced span may start or end mid-line; widen to whole lines so
    // every diff line is a real line of the file.
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = after
        .find('\n')
        .map(|i| range.end + i)
        .unwrap_or(contents.len());
    let old_block = &contents[line_start..line_end];
    let new_block = format!(
        "{}{}{}",
        &contents[line_start..range.start],
        new,
        &contents[range.end..line_end]
    );
    let pre: Vec<&str> = contents[..line_start].lines().collect();
    let post: Vec<&str> = contents[line_end..]
        .strip_prefix('\n')
        .unwrap_or(&contents[line_end..])
        .lines()
        .collect();
    let ctx_before = &pre[pre.len().saturating_sub(2)..];
    let ctx_after = &post[..post.len().min(2)];
    let old_lines: Vec<&str> = old_block.lines().collect();
    let new_lines: Vec<&str> = new_block.lines().collect();
    let first = pre.len() - ctx_before.len() + 1;
    let mut out = format!(
        "@@ -{},{} +{},{} @@\n",
        first,
        ctx_before.len() + old_lines.len() + ctx_after.len(),
        first,
        ctx_before.len() + new_lines.len() + ctx_after.len()
    );
    for l in ctx_before {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    for l in &old_lines {
        out.push('-');
        out.push_str(l);
        out.push('\n');
    }
    for l in &new_lines {
        out.push('+');
        out.push_str(l);
        out.push('\n');
    }
    for l in ctx_after {
        out.push(' ');
        out.push_str(l);
        out.push('\n');
    }
    out
}

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
                    Field::not_null("diff", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "replace one occurrence of `old` with `new` (exact, else ignoring whitespace and quotes) in a file read earlier; returns the diff"
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
        let range = locate(&contents, old).map_err(|e| match e {
            ToolError::Args(m) => ToolError::Args(format!("{path}: {m}")),
            other => other,
        })?;
        let diff = unified_diff(&contents, &range, new);
        let mut patched = String::with_capacity(contents.len() + new.len());
        patched.push_str(&contents[..range.start]);
        patched.push_str(new);
        patched.push_str(&contents[range.end..]);
        std::fs::write(&resolved.abs, patched)?;
        record_written(ctx, &resolved)?;
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![
                Value::Text(resolved.rel),
                Value::Int(1),
                Value::Text(diff),
            ]],
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
            "invalid arguments: f.txt: old text occurs 2 times; expected exactly one"
        );
        let err = Patch.call(&patch_args("q", "z"), &ctx).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid arguments: f.txt: old text not found (0 occurrences, exact or ignoring whitespace and quotes)"
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

    #[tokio::test]
    async fn falls_back_to_whitespace_and_quote_insensitive_lines() {
        let (dir, ctx) = ws("def f():\n    return \u{201c}a\u{201d}   \nprint(f())\n");
        read(&ctx).await;
        let b = Patch
            .call(&patch_args("  return \"a\"", "    return 'b'"), &ctx)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "def f():\n    return 'b'\nprint(f())\n"
        );
        let diff = b.rows[0][2].render();
        assert!(diff.starts_with("@@ -1,3 +1,3 @@\n"), "{diff}");
        assert!(
            diff.contains("-    return \u{201c}a\u{201d}   \n"),
            "{diff}"
        );
        assert!(diff.contains("+    return 'b'\n"), "{diff}");
        assert!(diff.contains(" print(f())\n"), "{diff}");
    }

    #[tokio::test]
    async fn ambiguous_fuzzy_matches_are_refused() {
        let (_dir, ctx) = ws(" x \n  x\t\ny\n");
        read(&ctx).await;
        let err = Patch
            .call(&patch_args("   x", "z"), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("matches 2 places"), "{err}");
    }

    #[test]
    fn diff_widens_a_mid_line_span_to_whole_lines() {
        let contents = "a\nb\nhello world\nc\nd\ne\n";
        let start = contents.find("world").unwrap();
        let d = unified_diff(contents, &(start..start + 5), "there");
        assert_eq!(
            d,
            "@@ -1,5 +1,5 @@\n a\n b\n-hello world\n+hello there\n c\n d\n"
        );
    }
}
