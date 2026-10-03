//! Shared HTTP plumbing for the wire adapters: status-code mapping and the
//! retry policy. Both adapters route every request through [`send`].

use crate::types::ProviderError;
use std::error::Error as _;
use std::future::Future;
use std::time::Duration;

/// A `reqwest` error with its cause chain, so a failed connection says why
/// (DNS, TLS, a refused connection) instead of only "error sending request".
pub fn describe(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let part = s.to_string();
        if !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = s.source();
    }
    text
}

/// Backoff between retries: three retries at 250ms, 1s and 4s.
pub(crate) const DEFAULT_BACKOFF: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(4),
];

/// Never honour a `retry-after` longer than this.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Pull the human-readable message out of an error body. Both Anthropic and
/// OpenAI-compatible servers put it at `error.message`; anything else is
/// returned trimmed and truncated.
pub(crate) fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(m) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            return m.to_string();
        }
        if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
            return m.to_string();
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty error body".into();
    }
    let mut cut = trimmed.len().min(512);
    while !trimmed.is_char_boundary(cut) {
        cut -= 1;
    }
    trimmed[..cut].to_string()
}

/// Map a non-2xx response to a typed error, reading its body for the message.
pub(crate) async fn check_status(
    resp: reqwest::Response,
) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let body = resp.text().await.unwrap_or_default();
    let msg = error_message(&body);
    Err(match status.as_u16() {
        401 | 403 => ProviderError::Auth(msg),
        429 => ProviderError::RateLimited {
            retry_after_secs: retry_after,
        },
        s if s >= 500 => ProviderError::Network(format!("HTTP {s}: {msg}")),
        _ => ProviderError::BadRequest(msg),
    })
}

/// Rate limits and transport / 5xx failures are worth retrying; anything else
/// is a property of the request or the credentials.
pub(crate) fn retryable(e: &ProviderError) -> bool {
    matches!(
        e,
        ProviderError::RateLimited { .. } | ProviderError::Network(_)
    )
}

/// Run `op`, retrying retryable errors once per entry in `backoff`, sleeping
/// that entry (or the server's `retry-after`, whichever is longer, capped at
/// 30s) before each retry.
pub(crate) async fn with_retry<T, F, Fut>(
    backoff: &[Duration],
    mut op: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if retryable(&e) && attempt < backoff.len() => {
                let mut wait = backoff[attempt];
                if let ProviderError::RateLimited {
                    retry_after_secs: Some(s),
                } = &e
                {
                    wait = wait.max(Duration::from_secs(*s)).min(MAX_RETRY_AFTER);
                }
                tracing::warn!(
                    attempt,
                    wait_ms = wait.as_millis() as u64,
                    error = %e,
                    "retrying provider call"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Send a request built by `build`, with the retry policy applied to
/// transport errors, 429 and 5xx. Returns the successful response.
pub(crate) async fn send(
    backoff: &[Duration],
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, ProviderError> {
    with_retry(backoff, || {
        let build = &build;
        async move {
            let resp = build()
                .send()
                .await
                .map_err(|e| ProviderError::Network(describe(&e)))?;
            check_status(resp).await
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    const NO_WAIT: [Duration; 3] = [Duration::ZERO; 3];

    #[test]
    fn error_message_prefers_error_dot_message() {
        assert_eq!(
            error_message(r#"{"type":"error","error":{"type":"x","message":"nope"}}"#),
            "nope"
        );
        assert_eq!(error_message(r#"{"message":"m"}"#), "m");
        assert_eq!(error_message("  plain text  "), "plain text");
        assert_eq!(error_message(""), "empty error body");
    }

    #[tokio::test]
    async fn retries_retryable_errors_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out = with_retry(&NO_WAIT, || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(ProviderError::Network("flaky".into()))
                } else {
                    Ok(n)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(out, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_backoff_entries_are_spent() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let err = with_retry(&NO_WAIT, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::RateLimited {
                    retry_after_secs: None,
                })
            }
        })
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::RateLimited { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn does_not_retry_bad_request_or_auth() {
        for make in [
            (|| ProviderError::BadRequest("b".into())) as fn() -> ProviderError,
            || ProviderError::Auth("a".into()),
        ] {
            let calls = Arc::new(AtomicU32::new(0));
            let c = calls.clone();
            let err = with_retry(&NO_WAIT, || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(make())
                }
            })
            .await
            .unwrap_err();
            assert!(!retryable(&err));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}
