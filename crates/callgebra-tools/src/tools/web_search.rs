//! `web_search(q TEXT [, n BIGINT])`: the search surface, shape fixed now,
//! backend later.

use crate::args::{check_arity, opt_int, text};
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `web_search(q TEXT [, n BIGINT]) -> (rank BIGINT, title TEXT, url TEXT, snippet TEXT)`.
///
/// No backend is wired in M2: every call fails with
/// `no web search backend configured` after validating its arguments. The
/// column shape is part of the interface and will not change when a
/// provider-side or HTTP backend arrives.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebSearch;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

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
        "web search results for a query: rank, title, url, snippet (no backend configured yet)"
    }

    async fn call(&self, args: &[Value], _ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let _q = text(args, 0, "q")?;
        if let Some(n) = opt_int(args, 1, "n")? {
            if n <= 0 {
                return Err(ToolError::Args(format!("n must be positive, got {n}")));
            }
        }
        Err(ToolError::Other("no web search backend configured".into()))
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
        assert_eq!(
            WebSearch.schema().names(),
            vec!["rank", "title", "url", "snippet"]
        );
    }
}
