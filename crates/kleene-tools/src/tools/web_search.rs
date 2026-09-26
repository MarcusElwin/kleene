//! `web_search(q TEXT [, n BIGINT])`: web search over an HTTP backend.

use crate::args::{check_arity, opt_int, text};
use crate::{Signature, Tool, ToolContext, ToolError, WebSearchBackend};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// `web_search(q TEXT [, n BIGINT]) -> (rank BIGINT, title TEXT, url TEXT, snippet TEXT)`.
///
/// The backend is [`ToolContext::web_search`]: Brave Search
/// (`api.search.brave.com`) or Tavily (`api.tavily.com`), each with its own
/// key. Without one every call fails with `no web search backend
/// configured` after validating its arguments. `n` defaults to 10 and is
/// capped at 20. The column shape is part of the interface.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebSearch;

/// Results returned when `n` is not given.
pub const DEFAULT_RESULTS: i64 = 10;
/// Most results one call returns.
pub const MAX_RESULTS: i64 = 20;
/// Whole-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(20);

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

/// One hit, before it becomes a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Page title.
    pub title: String,
    /// Page URL.
    pub url: String,
    /// The service's summary of the page.
    pub snippet: String,
}

/// Read Brave's `web.results[*]` into hits; `n` caps them.
pub fn parse_brave(body: &serde_json::Value, n: usize) -> Vec<Hit> {
    body.pointer("/web/results")
        .and_then(|r| r.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    Some(Hit {
                        title: r.get("title")?.as_str().unwrap_or_default().to_string(),
                        url: r.get("url")?.as_str()?.to_string(),
                        snippet: r
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .take(n)
                .collect()
        })
        .unwrap_or_default()
}

/// Read Tavily's `results[*]` into hits; `n` caps them.
pub fn parse_tavily(body: &serde_json::Value, n: usize) -> Vec<Hit> {
    body.get("results")
        .and_then(|r| r.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    Some(Hit {
                        title: r.get("title")?.as_str().unwrap_or_default().to_string(),
                        url: r.get("url")?.as_str()?.to_string(),
                        snippet: r
                            .get("content")
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .take(n)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads a service's JSON reply into hits.
type Parser = fn(&serde_json::Value, usize) -> Vec<Hit>;

/// Ask the backend for `n` hits on `q`.
async fn search(backend: &WebSearchBackend, q: &str, n: i64) -> Result<Vec<Hit>, ToolError> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| ToolError::Other(format!("http client: {e}")))?;
    let http =
        |e: reqwest::Error| ToolError::Other(format!("web search ({}): {e}", backend.provider));
    let (resp, parse): (reqwest::Response, Parser) = match backend.provider.as_str() {
        "brave" => (
            client
                .get("https://api.search.brave.com/res/v1/web/search")
                .query(&[("q", q), ("count", &n.to_string())])
                .header("Accept", "application/json")
                .header("X-Subscription-Token", &backend.api_key)
                .send()
                .await
                .map_err(http)?,
            parse_brave,
        ),
        "tavily" => (
            client
                .post("https://api.tavily.com/search")
                .json(&serde_json::json!({
                    "api_key": backend.api_key,
                    "query": q,
                    "max_results": n,
                }))
                .send()
                .await
                .map_err(http)?,
            parse_tavily,
        ),
        other => {
            return Err(ToolError::Other(format!(
                "unknown web search provider {other:?}; use brave or tavily"
            )))
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let why = match status.as_u16() {
            401 | 403 => "the API key was rejected".to_string(),
            429 => "rate limited".to_string(),
            _ => "request failed".to_string(),
        };
        return Err(ToolError::Other(format!(
            "web search ({}): {why} (HTTP {})",
            backend.provider,
            status.as_u16()
        )));
    }
    let body: serde_json::Value = resp.json().await.map_err(http)?;
    Ok(parse(&body, n as usize))
}

/// Hits as rows of the tool's schema, ranked from 1.
pub fn hits_to_batch(schema: Arc<Schema>, hits: Vec<Hit>) -> Result<Batch, ToolError> {
    let rows = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| {
            vec![
                Value::Int(i as i64 + 1),
                Value::from(h.title),
                Value::from(h.url),
                Value::from(h.snippet),
            ]
        })
        .collect();
    Batch::try_new(schema, rows).map_err(|e| ToolError::Other(e.to_string()))
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("rank", DataType::Int),
                    Field::not_null("title", DataType::Text),
                    Field::not_null("url", DataType::Text),
                    Field::not_null("snippet", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "web search results for a query: rank, title, url, snippet (n defaults to 10, at most 20)"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let q = text(args, 0, "q")?;
        let n = match opt_int(args, 1, "n")? {
            Some(n) if n <= 0 => {
                return Err(ToolError::Args(format!("n must be positive, got {n}")));
            }
            Some(n) => n.min(MAX_RESULTS),
            None => DEFAULT_RESULTS,
        };
        if q.trim().is_empty() {
            return Err(ToolError::Args("q must not be empty".into()));
        }
        let Some(backend) = &ctx.web_search else {
            return Err(ToolError::Other("no web search backend configured".into()));
        };
        let hits = search(backend, q, n).await?;
        hits_to_batch(self.schema(), hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn reports_the_missing_backend() {
        let ctx = ToolContext::new(PathBuf::from("."));
        let err = WebSearch
            .call(&[Value::from("rust"), Value::Int(5)], &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no web search backend configured");
        let err = WebSearch.call(&[], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Args(_)));
        let err = WebSearch
            .call(&[Value::from("rust"), Value::Int(0)], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
        assert_eq!(
            WebSearch.schema().names(),
            vec!["rank", "title", "url", "snippet"]
        );
    }

    #[test]
    fn backend_validates_its_provider_and_hides_its_key() {
        assert!(WebSearchBackend::new("brave", "").is_none());
        assert!(WebSearchBackend::new("bing", "k").is_none());
        let b = WebSearchBackend::new(" Tavily ", " tvly-secret ").unwrap();
        assert_eq!(b.provider, "tavily");
        assert_eq!(b.api_key, "tvly-secret");
        let shown = format!("{b:?}");
        assert!(
            shown.contains("tavily") && !shown.contains("secret"),
            "{shown}"
        );
    }

    #[test]
    fn parses_both_shapes_into_ranked_rows() {
        let brave = serde_json::json!({
            "web": {"results": [
                {"title": "Rust", "url": "https://rust-lang.org", "description": "A language"},
                {"title": "Crates", "url": "https://crates.io"},
                {"title": "no url"}
            ]}
        });
        let hits = parse_brave(&brave, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].snippet, "");
        assert_eq!(parse_brave(&brave, 1).len(), 1);
        let tavily = serde_json::json!({
            "results": [{"title": "T", "url": "https://t.example", "content": "c"}]
        });
        let hits = parse_tavily(&tavily, 5);
        assert_eq!(hits[0].snippet, "c");
        assert!(parse_tavily(&serde_json::json!({}), 5).is_empty());
        let batch = hits_to_batch(WebSearch.schema(), hits).unwrap();
        assert_eq!(batch.len(), 1);
        let header = batch.render_table(10).lines().next().unwrap().to_string();
        let names: Vec<&str> = header.split('|').map(str::trim).collect();
        assert_eq!(names, ["rank", "title", "url", "snippet"]);
    }
}
