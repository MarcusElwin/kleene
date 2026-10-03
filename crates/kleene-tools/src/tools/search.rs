//! `search(query TEXT [, glob TEXT, n BIGINT])`: ranked full-text search
//! over workspace files.

use crate::args::{check_arity, opt_int, opt_text, text};
use crate::tools::files::walk;
use crate::tools::grep::{looks_binary, MAX_FILE_BYTES};
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Results returned when `n` is not given.
pub const DEFAULT_RESULTS: i64 = 10;
/// The most results one call returns.
pub const MAX_RESULTS: i64 = 100;

/// `search(query TEXT [, glob TEXT, n BIGINT]) -> (path TEXT, score DOUBLE, lineno BIGINT, snippet TEXT)`.
///
/// Scores every text file matching `glob` (default `**`) against the query
/// with BM25 over lower-cased alphanumeric tokens and returns the best `n`
/// (default 10, at most 100), each with the line that carries the most
/// query terms as its snippet. Binary files, files over 4 MiB and the
/// directories `files` skips are left out. Where `grep` wants an exact
/// regex, `search` takes words and ranks: ask for the concept, then open
/// the file with `lines` at the line it points to.
#[derive(Debug, Default, Clone, Copy)]
pub struct Search;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

/// Lower-cased runs of letters, digits and underscores.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Doc {
    rel: String,
    text: String,
    len: usize,
    tf: HashMap<String, usize>,
}

/// BM25 scores for `terms` over `docs`, with k1 = 1.2 and b = 0.75.
fn bm25(docs: &[Doc], terms: &[String]) -> Vec<f64> {
    let n = docs.len() as f64;
    if n == 0.0 {
        return vec![];
    }
    let avg = docs.iter().map(|d| d.len).sum::<usize>() as f64 / n;
    let (k1, b) = (1.2f64, 0.75f64);
    let df: HashMap<&str, f64> = terms
        .iter()
        .map(|t| {
            (
                t.as_str(),
                docs.iter().filter(|d| d.tf.contains_key(t)).count() as f64,
            )
        })
        .collect();
    docs.iter()
        .map(|d| {
            terms
                .iter()
                .map(|t| {
                    let f = *d.tf.get(t).unwrap_or(&0) as f64;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let idf = ((n - df[t.as_str()] + 0.5) / (df[t.as_str()] + 0.5) + 1.0).ln();
                    idf * (f * (k1 + 1.0)) / (f + k1 * (1.0 - b + b * d.len as f64 / avg.max(1.0)))
                })
                .sum()
        })
        .collect()
}

/// The 1-based line with the most distinct query terms, and the line itself.
fn best_line(text: &str, terms: &[String]) -> (i64, String) {
    let mut best = (0usize, 1i64, String::new());
    for (i, line) in text.lines().enumerate() {
        let toks = tokens(line);
        let hits = terms.iter().filter(|t| toks.contains(t)).count();
        if hits > best.0 {
            best = (hits, i as i64 + 1, line.trim().chars().take(160).collect());
        }
    }
    (best.1, best.2)
}

#[async_trait::async_trait]
impl Tool for Search {
    fn name(&self) -> &str {
        "search"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[DataType::Text, DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("path", DataType::Text),
                    Field::not_null("score", DataType::Float),
                    Field::not_null("lineno", DataType::Int),
                    Field::not_null("snippet", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "workspace files ranked by BM25 against the query's words: path, score, the best line and its text (n defaults to 10)"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let query = text(args, 0, "query")?;
        let glob = opt_text(args, 1, "glob")?.unwrap_or("**");
        let n = opt_int(args, 2, "n")?.unwrap_or(DEFAULT_RESULTS);
        if n < 1 {
            return Err(ToolError::Args(format!("n must be positive, got {n}")));
        }
        let n = n.min(MAX_RESULTS) as usize;
        let terms = tokens(query);
        if terms.is_empty() {
            return Err(ToolError::Args("query has no words".into()));
        }
        let mut docs = vec![];
        for entry in walk(ctx, glob)? {
            if !entry.meta.is_file() || entry.meta.len() > MAX_FILE_BYTES {
                continue;
            }
            let bytes = std::fs::read(&entry.abs)?;
            if looks_binary(&bytes) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let toks = tokens(&text);
            let mut tf = HashMap::new();
            for t in &toks {
                if terms.contains(t) {
                    *tf.entry(t.clone()).or_insert(0) += 1;
                }
            }
            docs.push(Doc {
                rel: entry.rel,
                len: toks.len(),
                tf,
                text,
            });
        }
        let scores = bm25(&docs, &terms);
        let mut ranked: Vec<(usize, f64)> = scores
            .iter()
            .enumerate()
            .filter(|(_, s)| **s > 0.0)
            .map(|(i, s)| (i, *s))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| docs[a.0].rel.cmp(&docs[b.0].rel))
        });
        let rows = ranked
            .into_iter()
            .take(n)
            .map(|(i, s)| {
                let (lineno, snippet) = best_line(&docs[i].text, &terms);
                vec![
                    Value::Text(docs[i].rel.clone()),
                    Value::Float((s * 1000.0).round() / 1000.0),
                    Value::Int(lineno),
                    Value::Text(snippet),
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
        std::fs::write(
            dir.path().join("parse.py"),
            "def parse_transactions(text):\n    # amounts may have thousands separators\n    return rows\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("report.py"),
            "def report(rows):\n    total = sum(r['amount'] for r in rows)\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "nothing relevant here\n").unwrap();
        std::fs::write(dir.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        (dir, ctx)
    }

    #[tokio::test]
    async fn ranks_the_file_with_the_query_words_first() {
        let (_dir, ctx) = ws();
        let b = Search
            .call(&[Value::from("thousands separators amount")], &ctx)
            .await
            .unwrap();
        let paths: Vec<String> = b.rows.iter().map(|r| r[0].render()).collect();
        assert_eq!(paths, vec!["parse.py", "report.py"]);
        assert_eq!(b.rows[0][2], Value::Int(2));
        assert_eq!(
            b.rows[0][3].render(),
            "# amounts may have thousands separators"
        );
        let score = |v: &Value| match v {
            Value::Float(f) => *f,
            other => panic!("not a float: {other:?}"),
        };
        assert!(score(&b.rows[0][1]) > score(&b.rows[1][1]));
    }

    #[tokio::test]
    async fn glob_and_n_narrow_the_result() {
        let (_dir, ctx) = ws();
        let b = Search
            .call(
                &[Value::from("rows"), Value::from("*.py"), Value::Int(1)],
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(b.rows.len(), 1);
        let err = Search.call(&[Value::from("!!!")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)));
    }

    #[test]
    fn tokens_are_lowercase_words() {
        assert_eq!(
            tokens("Parse_Transactions(text) -> 8 rows!"),
            vec!["parse_transactions", "text", "8", "rows"]
        );
    }
}
