//! `web_fetch(url TEXT)`: GET a page from an allow-listed host.

use crate::args::{check_arity, text, tokens};
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Most body bytes kept; the rest of the response is discarded.
pub const MAX_BODY_BYTES: usize = 200 * 1024;
/// Whole-request timeout.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Tags after which a line break is inserted when stripping HTML.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "hr",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "main",
    "title",
    "dt",
    "dd",
];

/// Reduce HTML to readable text: drop `<script>`, `<style>` and comments,
/// turn block-level tags into line breaks, strip every other tag, decode
/// the common entities, and collapse whitespace (runs of spaces to one,
/// blank lines dropped).
pub fn strip_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let ch_end = next_char_boundary(html, i);
            out.push_str(&html[i..ch_end]);
            i = ch_end;
            continue;
        }
        if lower[i..].starts_with("<!--") {
            i = lower[i..].find("-->").map_or(bytes.len(), |p| i + p + 3);
            continue;
        }
        let close = match lower[i..].find('>') {
            Some(p) => i + p,
            None => break,
        };
        let inner = lower[i + 1..close].trim_start_matches('/');
        let name: String = inner
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if (name == "script" || name == "style") && !lower[i + 1..].starts_with('/') {
            let end_tag = format!("</{name}");
            i = match lower[close..].find(&end_tag) {
                Some(p) => {
                    let at = close + p;
                    lower[at..].find('>').map_or(bytes.len(), |q| at + q + 1)
                }
                None => bytes.len(),
            };
            out.push('\n');
            continue;
        }
        if BLOCK_TAGS.contains(&name.as_str()) {
            out.push('\n');
        } else {
            out.push(' ');
        }
        i = close + 1;
    }
    collapse_whitespace(&decode_entities(&out))
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded: Option<String> = match entity {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" => Some("'".into()),
            "nbsp" => Some(" ".into()),
            e if e.starts_with('#') => {
                let num = &e[1..];
                let code = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    num.parse::<u32>().ok()
                };
                code.and_then(char::from_u32).map(String::from)
            }
            _ => None,
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse_whitespace(s: &str) -> String {
    s.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `web_fetch(url TEXT) -> (url TEXT, status BIGINT, text TEXT, tokens BIGINT)`.
///
/// A GET with a 20 s timeout. The URL's host must be on
/// [`ToolContext::network_allowlist`] (exact, case-insensitive); anything
/// else is denied before any connection is made. The body is cut at 200 KiB;
/// HTML responses are reduced to text with [`strip_html`]. `tokens` is
/// `ceil(chars / 4)` of the returned text. Volatile: the web changes.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebFetch;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("url", DataType::Text),
                    Field::not_null("status", DataType::Int),
                    Field::not_null("text", DataType::Text),
                    Field::not_null("tokens", DataType::Int),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "GET a URL on an allow-listed host: status and body as text (HTML stripped), cut at 200 KiB"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let url = text(args, 0, "url")?;
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| ToolError::Args(format!("bad url {url:?}: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ToolError::Args(format!(
                "unsupported scheme {}; use http or https",
                parsed.scheme()
            )));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| ToolError::Args(format!("{url} has no host")))?;
        if !ctx
            .network_allowlist
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
        {
            return Err(ToolError::Denied(format!(
                "host {host} is not on the network allow-list"
            )));
        }

        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| ToolError::Other(format!("http client: {e}")))?;
        let mut resp = client
            .get(parsed.clone())
            .send()
            .await
            .map_err(|e| ToolError::Other(format!("fetch {url}: {e}")))?;
        let status = i64::from(resp.status().as_u16());
        let is_html = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("html"));
        let mut body: Vec<u8> = Vec::new();
        while body.len() < MAX_BODY_BYTES {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    let room = MAX_BODY_BYTES - body.len();
                    body.extend_from_slice(&chunk[..chunk.len().min(room)]);
                }
                Ok(None) => break,
                Err(e) => return Err(ToolError::Other(format!("fetch {url}: {e}"))),
            }
        }
        let raw = String::from_utf8_lossy(&body);
        let looks_html = {
            let head = raw
                .trim_start()
                .get(..15)
                .unwrap_or("")
                .to_ascii_lowercase();
            head.starts_with("<!doctype") || head.starts_with("<html")
        };
        let body_text = if is_html || looks_html {
            strip_html(&raw)
        } else {
            raw.into_owned()
        };
        let n = body_text.chars().count();
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![
                Value::Text(parsed.to_string()),
                Value::Int(status),
                Value::Text(body_text),
                Value::Int(tokens(n)),
            ]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn strips_tags_scripts_and_entities() {
        let html = "<!DOCTYPE html><html><head><title>T</title>\
            <style>body{color:red}</style><script>var x = '<p>';</script></head>\
            <body><h1>Hello   &amp; <b>welcome</b></h1>\n\n\n<p>One&nbsp;two&#65;&#x42;</p>\
            <!-- hidden --><ul><li>a</li><li>b</li></ul>&unknown; tail</body></html>";
        assert_eq!(
            strip_html(html),
            "T\nHello & welcome\nOne twoAB\na\nb\n&unknown; tail"
        );
        assert_eq!(strip_html("plain text"), "plain text");
        assert_eq!(strip_html("<p>unterminated <b"), "unterminated");
    }

    #[tokio::test]
    async fn denies_hosts_off_the_allow_list_without_a_network() {
        let mut ctx = ToolContext::new(PathBuf::from("."));
        let err = WebFetch
            .call(&[Value::from("https://example.com/x")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        assert!(err.to_string().contains("example.com"));
        ctx.network_allowlist = vec!["docs.rs".into()];
        let err = WebFetch
            .call(&[Value::from("https://EXAMPLE.com/")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
    }

    #[tokio::test]
    async fn rejects_bad_urls_and_schemes() {
        let mut ctx = ToolContext::new(PathBuf::from("."));
        ctx.network_allowlist = vec!["example.com".into()];
        let err = WebFetch
            .call(&[Value::from("not a url")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
        let err = WebFetch
            .call(&[Value::from("file:///etc/passwd")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
        let err = WebFetch
            .call(&[Value::from("ftp://example.com/x")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
    }
}
