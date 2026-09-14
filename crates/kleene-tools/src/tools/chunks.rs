//! `chunks(text TEXT, size BIGINT [, overlap BIGINT])`: split text into
//! overlapping windows, the partitioning step before a `LATERAL RLM(...)`.

use crate::args::{check_arity, int, opt_int, opt_text, tokens};
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// Split `text` into chunks of at most `size` characters, each starting
/// `overlap` characters before the previous one ended. When a window is not
/// the last one and contains a newline in its final fifth, the chunk ends
/// just after the last such newline, so lines are not cut mid-way when a
/// nearby break exists.
pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = (start + size).min(n);
        if end < n {
            let window = (size / 5).max(1);
            let lo = end - window;
            if let Some(pos) = (lo..end).rev().find(|&i| chars[i] == '\n') {
                end = pos + 1;
            }
        }
        out.push(chars[start..end].iter().collect());
        if end >= n {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }
    out
}

/// `chunks(text TEXT, size BIGINT [, overlap BIGINT]) -> (ordinal BIGINT, text TEXT, tokens BIGINT)`.
///
/// `ordinal` is 0-based, `tokens` is `ceil(chars / 4)`. `overlap` defaults to
/// 0 and must be smaller than `size`. A `NULL` or empty text yields no rows.
/// Pure, so `IMMUTABLE`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Chunks;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Chunks {
    fn name(&self) -> &str {
        "chunks"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text, DataType::Int], &[DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("ordinal", DataType::Int),
                    Field::not_null("text", DataType::Text),
                    Field::not_null("tokens", DataType::Int),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Immutable
    }

    fn description(&self) -> &str {
        "text split into windows of `size` characters with `overlap`, breaking at newlines when near"
    }

    async fn call(&self, args: &[Value], _ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let text = opt_text(args, 0, "text")?;
        let size = int(args, 1, "size")?;
        if size <= 0 {
            return Err(ToolError::Args(format!(
                "size must be positive, got {size}"
            )));
        }
        let overlap = opt_int(args, 2, "overlap")?.unwrap_or(0);
        if overlap < 0 || overlap >= size {
            return Err(ToolError::Args(format!(
                "overlap must be between 0 and size - 1, got {overlap} for size {size}"
            )));
        }
        let rows = match text {
            None => Vec::new(),
            Some(t) => chunk_text(t, size as usize, overlap as usize)
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    let n = c.chars().count();
                    vec![Value::Int(i as i64), Value::Text(c), Value::Int(tokens(n))]
                })
                .collect(),
        };
        Ok(Batch {
            schema: self.schema(),
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("."))
    }

    #[test]
    fn plain_windows_with_overlap() {
        assert_eq!(chunk_text("abcdefghij", 4, 0), vec!["abcd", "efgh", "ij"]);
        assert_eq!(chunk_text("abcdefghij", 4, 1), vec!["abcd", "defg", "ghij"]);
        assert_eq!(chunk_text("", 4, 0), Vec::<String>::new());
        assert_eq!(chunk_text("ab", 4, 0), vec!["ab"]);
    }

    #[test]
    fn prefers_a_newline_in_the_last_fifth() {
        // Window of 10: the newline at index 8 is inside the last fifth (8..10).
        assert_eq!(
            chunk_text("12345678\nabcdefghij", 10, 0),
            vec!["12345678\n", "abcdefghij"]
        );
        // A newline outside the last fifth is ignored.
        assert_eq!(
            chunk_text("12\n45678901234567", 10, 0),
            vec!["12\n4567890", "1234567"]
        );
        // The last window never shortens.
        assert_eq!(chunk_text("abc\nd", 10, 0), vec!["abc\nd"]);
    }

    #[test]
    fn overlap_applies_after_a_newline_break() {
        assert_eq!(
            chunk_text("12345678\nabcdefghij", 10, 2),
            vec!["12345678\n", "8\nabcdefgh", "ghij"]
        );
    }

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(chunk_text("ééééé", 2, 0), vec!["éé", "éé", "é"]);
    }

    #[tokio::test]
    async fn rows_carry_ordinal_and_tokens() {
        let b = Chunks
            .call(
                &[Value::from("abcdefghij"), Value::Int(4), Value::Int(1)],
                &ctx(),
            )
            .await
            .unwrap();
        let got: Vec<(i64, String, i64)> = b
            .rows
            .iter()
            .map(|r| {
                (
                    r[0].as_int().unwrap(),
                    r[1].render(),
                    r[2].as_int().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (0, "abcd".into(), 1),
                (1, "defg".into(), 1),
                (2, "ghij".into(), 1)
            ]
        );
        let b = Chunks
            .call(&[Value::from("abcde"), Value::Int(5)], &ctx())
            .await
            .unwrap();
        assert_eq!(b.rows[0][2], Value::Int(2));
        let b = Chunks
            .call(&[Value::Null, Value::Int(5)], &ctx())
            .await
            .unwrap();
        assert!(b.is_empty());
    }

    #[tokio::test]
    async fn validates_size_and_overlap() {
        let c = ctx();
        assert!(Chunks
            .call(&[Value::from("x"), Value::Int(0)], &c)
            .await
            .is_err());
        assert!(Chunks
            .call(&[Value::from("x"), Value::Int(3), Value::Int(3)], &c)
            .await
            .is_err());
        assert!(Chunks
            .call(&[Value::from("x"), Value::Int(3), Value::Int(-1)], &c)
            .await
            .is_err());
        assert!(Chunks.call(&[Value::from("x")], &c).await.is_err());
    }
}
