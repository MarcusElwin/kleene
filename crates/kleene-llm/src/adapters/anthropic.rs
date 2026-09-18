//! Anthropic Messages API adapter (`POST {base}/v1/messages`), spoken
//! directly over `reqwest` in the current wire format: adaptive thinking,
//! `output_config` for effort and JSON-schema output, `cache_control` on the
//! system prefix, and `refusal` as a stop reason rather than an error.

use crate::http::{self, DEFAULT_BACKOFF};
use crate::sse::{self, Assembler, SseEvent};
use crate::types::{
    Capabilities, CompletionRequest, CompletionResponse, ContentBlock, Message, ProviderError,
    Role, StopReason, StreamEvent, Usage,
};
use crate::Provider;
use bytes::Bytes;
use futures::stream::{BoxStream, Stream};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fmt::Display;
use std::time::Duration;

/// Default API base.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// API version header sent with every request.
pub const API_VERSION: &str = "2023-06-01";
/// Beta header required when authenticating with an OAuth token.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// How the request is authenticated.
#[derive(Clone)]
enum Auth {
    /// `x-api-key`.
    ApiKey(String),
    /// `Authorization: Bearer` plus the OAuth beta header.
    Bearer(String),
}

/// Anthropic Messages API backend.
pub struct AnthropicProvider {
    client: reqwest::Client,
    auth: Auth,
    base_url: String,
    backoff: Vec<Duration>,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("base_url", &self.base_url)
            .field(
                "auth",
                &match self.auth {
                    Auth::ApiKey(_) => "api_key",
                    Auth::Bearer(_) => "bearer",
                },
            )
            .finish()
    }
}

impl AnthropicProvider {
    /// Authenticate with an API key (`x-api-key`). `base_url` defaults to
    /// [`DEFAULT_BASE_URL`]; a trailing slash is tolerated.
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self::build(Auth::ApiKey(api_key), base_url)
    }

    /// Authenticate with an OAuth access token (`Authorization: Bearer`
    /// plus the `oauth-2025-04-20` beta header).
    pub fn with_auth_token(token: String, base_url: Option<String>) -> Self {
        Self::build(Auth::Bearer(token), base_url)
    }

    /// Read `ANTHROPIC_API_KEY` (preferred) or `ANTHROPIC_AUTH_TOKEN`, and
    /// `ANTHROPIC_BASE_URL`. Errors with [`ProviderError::Auth`] when neither
    /// credential is set.
    pub fn from_env() -> Result<Self, ProviderError> {
        let base = non_empty_env("ANTHROPIC_BASE_URL");
        if let Some(key) = non_empty_env("ANTHROPIC_API_KEY") {
            return Ok(Self::new(key, base));
        }
        if let Some(token) = non_empty_env("ANTHROPIC_AUTH_TOKEN") {
            return Ok(Self::with_auth_token(token, base));
        }
        Err(ProviderError::Auth(
            "neither ANTHROPIC_API_KEY nor ANTHROPIC_AUTH_TOKEN is set".into(),
        ))
    }

    /// Override the retry backoff schedule (one retry per entry).
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }

    /// The API base this adapter talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn build(auth: Auth, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            auth,
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff: DEFAULT_BACKOFF.to_vec(),
        }
    }

    /// Whether the `thinking` parameter is sent for this model. Haiku models
    /// do not accept adaptive thinking, so it is omitted for them.
    pub fn supports_thinking(model: &str) -> bool {
        !model.starts_with("claude-haiku")
    }

    /// The JSON body sent for a request (without `stream`). Public so the
    /// mapping is testable and can be logged by the trace.
    pub fn build_body(req: &CompletionRequest) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        body.insert("max_tokens".into(), json!(req.max_tokens));
        if !req.system.is_empty() {
            let mut block = json!({"type": "text", "text": req.system});
            if req.options.cache_prefix {
                block["cache_control"] = json!({"type": "ephemeral"});
            }
            body.insert("system".into(), Value::Array(vec![block]));
        }
        body.insert(
            "messages".into(),
            Value::Array(req.messages.iter().map(message_to_wire).collect()),
        );
        if !req.tools.is_empty() {
            let tools = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        let mut output_config = Map::new();
        if let Some(schema) = &req.output_schema {
            output_config.insert(
                "format".into(),
                json!({"type": "json_schema", "schema": schema}),
            );
        }
        if let Some(effort) = &req.options.effort {
            output_config.insert("effort".into(), json!(effort));
        }
        if !output_config.is_empty() {
            body.insert("output_config".into(), Value::Object(output_config));
        }
        if Self::supports_thinking(&req.model) {
            body.insert("thinking".into(), json!({"type": "adaptive"}));
        }
        if let Some(t) = req.options.temperature {
            body.insert("temperature".into(), super::f32_json(t));
        }
        for (k, v) in &req.options.extra {
            body.insert(k.clone(), v.clone());
        }
        Value::Object(body)
    }

    /// Parse a non-streaming response body.
    pub fn parse_response(body: &str) -> Result<CompletionResponse, ProviderError> {
        let msg: ApiMessage =
            serde_json::from_str(body).map_err(|e| ProviderError::Malformed(e.to_string()))?;
        let content = msg
            .content
            .into_iter()
            .filter_map(|b| match b {
                ApiBlock::Text { text } => Some(ContentBlock::Text { text }),
                ApiBlock::ToolUse { id, name, input } => {
                    Some(ContentBlock::ToolUse { id, name, input })
                }
                ApiBlock::Other => None,
            })
            .collect();
        Ok(CompletionResponse {
            model: msg.model,
            content,
            stop_reason: map_stop_reason(msg.stop_reason.as_deref()),
            usage: msg.usage.into_usage(),
        })
    }

    /// Decode an SSE byte stream (the body of a `stream: true` request) into
    /// provider-neutral events, ending with [`StreamEvent::Done`]. Fixture
    /// transcripts and live responses take the same path.
    pub fn decode_stream<S, E>(bytes: S) -> BoxStream<'static, Result<StreamEvent, ProviderError>>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Display + Send + 'static,
    {
        sse::assemble(bytes, MessageAssembler::default())
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let r = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");
        let r = match &self.auth {
            Auth::ApiKey(k) => r.header("x-api-key", k),
            Auth::Bearer(t) => r.bearer_auth(t).header("anthropic-beta", OAUTH_BETA),
        };
        r.json(body)
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn message_to_wire(m: &Message) -> Value {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = m
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
        })
        .collect();
    json!({"role": role, "content": content})
}

fn map_stop_reason(s: Option<&str>) -> StopReason {
    match s {
        Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("tool_use") => StopReason::ToolUse,
        Some("refusal") => StopReason::Refusal,
        _ => StopReason::Other,
    }
}

#[derive(Deserialize)]
struct ApiMessage {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<ApiBlock>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: ApiUsage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

impl ApiUsage {
    fn into_usage(self) -> Usage {
        Usage {
            input_tokens: self.input_tokens.unwrap_or(0),
            output_tokens: self.output_tokens.unwrap_or(0),
            cache_read_tokens: self.cache_read_input_tokens.unwrap_or(0),
            cache_write_tokens: self.cache_creation_input_tokens.unwrap_or(0),
            cost_usd: None,
        }
    }

    /// Overlay the fields present in a later usage report (`message_delta`
    /// carries cumulative counts for whatever it includes).
    fn merge_into(self, u: &mut Usage) {
        if let Some(n) = self.input_tokens {
            u.input_tokens = n;
        }
        if let Some(n) = self.output_tokens {
            u.output_tokens = n;
        }
        if let Some(n) = self.cache_read_input_tokens {
            u.cache_read_tokens = n;
        }
        if let Some(n) = self.cache_creation_input_tokens {
            u.cache_write_tokens = n;
        }
    }
}

/// A content block under construction while streaming.
enum PartialBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
}

/// Assembles `message_start` … `message_stop` into a response.
#[derive(Default)]
struct MessageAssembler {
    started: bool,
    model: String,
    blocks: Vec<PartialBlock>,
    usage: Usage,
    stop_reason: StopReason,
}

impl MessageAssembler {
    fn block_mut(&mut self, index: usize) -> Result<&mut PartialBlock, ProviderError> {
        self.blocks.get_mut(index).ok_or_else(|| {
            ProviderError::Malformed(format!("delta for unknown content block {index}"))
        })
    }

    fn finish_response(&mut self) -> Result<CompletionResponse, ProviderError> {
        let mut content = Vec::with_capacity(self.blocks.len());
        for b in self.blocks.drain(..) {
            content.push(match b {
                PartialBlock::Text(text) => ContentBlock::Text { text },
                PartialBlock::ToolUse { id, name, json } => {
                    let input = if json.trim().is_empty() {
                        Value::Object(Map::new())
                    } else {
                        serde_json::from_str(&json).map_err(|e| {
                            ProviderError::Malformed(format!("tool input for {name}: {e}"))
                        })?
                    };
                    ContentBlock::ToolUse { id, name, input }
                }
            });
        }
        Ok(CompletionResponse {
            model: std::mem::take(&mut self.model),
            content,
            stop_reason: self.stop_reason,
            usage: self.usage,
        })
    }
}

impl Assembler for MessageAssembler {
    fn on_event(&mut self, ev: SseEvent) -> Result<Vec<StreamEvent>, ProviderError> {
        let data: Value = serde_json::from_str(&ev.data)
            .map_err(|e| ProviderError::Malformed(format!("stream event: {e}")))?;
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(ev.event)
            .unwrap_or_default();
        let mut out = Vec::new();
        match kind.as_str() {
            "message_start" => {
                self.started = true;
                let msg = data.get("message").cloned().unwrap_or(Value::Null);
                self.model = msg
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(u) = msg.get("usage") {
                    let u: ApiUsage = serde_json::from_value(u.clone()).unwrap_or_default();
                    u.merge_into(&mut self.usage);
                }
            }
            "content_block_start" => {
                let block = data.get("content_block").cloned().unwrap_or(Value::Null);
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if !text.is_empty() {
                            out.push(StreamEvent::TextDelta { text: text.clone() });
                        }
                        self.blocks.push(PartialBlock::Text(text));
                    }
                    Some("tool_use") => {
                        let id = str_field(&block, "id");
                        let name = str_field(&block, "name");
                        out.push(StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        self.blocks.push(PartialBlock::ToolUse {
                            id,
                            name,
                            json: String::new(),
                        });
                    }
                    // Thinking and other block kinds are not surfaced; keep a
                    // placeholder so indices stay aligned.
                    _ => self.blocks.push(PartialBlock::Text(String::new())),
                }
            }
            "content_block_delta" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = data.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = str_field(&delta, "text");
                        if let PartialBlock::Text(t) = self.block_mut(index)? {
                            t.push_str(&text);
                        }
                        out.push(StreamEvent::TextDelta { text });
                    }
                    Some("input_json_delta") => {
                        let partial_json = str_field(&delta, "partial_json");
                        if partial_json.is_empty() {
                            return Ok(out);
                        }
                        if let PartialBlock::ToolUse { json, .. } = self.block_mut(index)? {
                            json.push_str(&partial_json);
                        }
                        out.push(StreamEvent::ToolInputDelta { partial_json });
                    }
                    _ => {}
                }
            }
            "content_block_stop" | "ping" => {}
            "message_delta" => {
                if let Some(s) = data.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = map_stop_reason(Some(s));
                }
                if let Some(u) = data.get("usage") {
                    let u: ApiUsage = serde_json::from_value(u.clone()).unwrap_or_default();
                    u.merge_into(&mut self.usage);
                }
            }
            "message_stop" => {
                out.push(StreamEvent::Done(self.finish_response()?));
            }
            "error" => {
                let msg = data
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown stream error");
                return Err(ProviderError::Other(msg.to_string()));
            }
            other => tracing::debug!(event = other, "ignoring unknown stream event"),
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        if self.started {
            Err(ProviderError::Malformed(
                "stream ended before message_stop".into(),
            ))
        } else {
            Err(ProviderError::Malformed("empty stream".into()))
        }
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            json_schema: true,
            prompt_cache: true,
            reasoning_control: true,
            cost_reported: false,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let body = Self::build_body(&req);
        let resp = http::send(&self.backoff, || self.request(&body)).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Self::parse_response(&text)
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        let mut body = Self::build_body(&req);
        body["stream"] = json!(true);
        let resp = http::send(&self.backoff, || self.request(&body)).await?;
        Ok(Self::decode_stream(resp.bytes_stream()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProviderOptions, ToolDef};
    use futures::StreamExt;
    use kleene_core::ModelAlias;
    use std::convert::Infallible;

    fn req() -> CompletionRequest {
        CompletionRequest {
            alias: ModelAlias::worker(),
            model: "claude-sonnet-5".into(),
            system: "You are terse.".into(),
            messages: vec![
                Message::user("hi"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text {
                            text: "let me look".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "toolu_1".into(),
                            name: "ls".into(),
                            input: json!({"path": "."}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "toolu_1".into(),
                        content: "permission denied".into(),
                        is_error: true,
                    }],
                },
            ],
            tools: vec![ToolDef {
                name: "ls".into(),
                description: "list".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
            output_schema: Some(
                json!({"type": "object", "properties": {"ok": {"type": "boolean"}}}),
            ),
            max_tokens: 512,
            options: ProviderOptions {
                effort: Some("low".into()),
                cache_prefix: true,
                temperature: None,
                extra: Map::new(),
            },
        }
    }

    #[test]
    fn body_maps_every_field() {
        let body = AnthropicProvider::build_body(&req());
        assert_eq!(body["model"], "claude-sonnet-5");
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(
            body["system"],
            json!([{"type": "text", "text": "You are terse.", "cache_control": {"type": "ephemeral"}}])
        );
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "let me look"},
                    {"type": "tool_use", "id": "toolu_1", "name": "ls", "input": {"path": "."}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "permission denied", "is_error": true}
                ]}
            ])
        );
        assert_eq!(
            body["tools"],
            json!([{"name": "ls", "description": "list",
                    "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}])
        );
        assert_eq!(
            body["output_config"],
            json!({
                "format": {"type": "json_schema",
                           "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}},
                "effort": "low"
            })
        );
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert!(body.get("temperature").is_none());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn body_omits_optional_parts_and_merges_extra() {
        let mut r = req();
        r.system.clear();
        r.tools.clear();
        r.output_schema = None;
        r.options = ProviderOptions {
            effort: None,
            cache_prefix: false,
            temperature: Some(0.2),
            extra: [("metadata".to_string(), json!({"user_id": "u1"}))]
                .into_iter()
                .collect(),
        };
        let body = AnthropicProvider::build_body(&r);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("output_config").is_none());
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["metadata"], json!({"user_id": "u1"}));
    }

    #[test]
    fn system_without_cache_prefix_has_no_cache_control() {
        let mut r = req();
        r.options.cache_prefix = false;
        let body = AnthropicProvider::build_body(&r);
        assert_eq!(
            body["system"],
            json!([{"type": "text", "text": "You are terse."}])
        );
    }

    #[test]
    fn thinking_is_omitted_for_haiku() {
        let mut r = req();
        r.model = "claude-haiku-4-5".into();
        let body = AnthropicProvider::build_body(&r);
        assert!(body.get("thinking").is_none());
        r.model = "claude-opus-5".into();
        assert!(AnthropicProvider::build_body(&r).get("thinking").is_some());
    }

    #[test]
    fn parses_text_response_with_cache_usage() {
        let resp = AnthropicProvider::parse_response(include_str!(
            "../../tests/fixtures/anthropic_text.json"
        ))
        .unwrap();
        assert_eq!(resp.model, "claude-sonnet-5");
        assert_eq!(resp.text(), "Hello there.");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 12,
                output_tokens: 5,
                cache_read_tokens: 1000,
                cache_write_tokens: 200,
                cost_usd: None,
            }
        );
    }

    #[test]
    fn parses_tool_use_response_and_skips_thinking_blocks() {
        let resp = AnthropicProvider::parse_response(include_str!(
            "../../tests/fixtures/anthropic_tool_use.json"
        ))
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(
            resp.content,
            vec![
                ContentBlock::Text {
                    text: "Checking.".into()
                },
                ContentBlock::ToolUse {
                    id: "toolu_01".into(),
                    name: "ls".into(),
                    input: json!({"path": "/tmp"}),
                }
            ]
        );
    }

    #[test]
    fn refusal_is_a_stop_reason_not_an_error() {
        let resp = AnthropicProvider::parse_response(include_str!(
            "../../tests/fixtures/anthropic_refusal.json"
        ))
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::Refusal);
        assert!(resp.content.is_empty());
    }

    #[test]
    fn malformed_body_is_reported() {
        assert!(matches!(
            AnthropicProvider::parse_response("{not json"),
            Err(ProviderError::Malformed(_))
        ));
    }

    /// Split a transcript into small, uneven chunks so line reassembly is
    /// exercised on the real code path.
    fn chunked(transcript: &'static str) -> impl Stream<Item = Result<Bytes, Infallible>> {
        let bytes = transcript.as_bytes();
        let mut chunks = Vec::new();
        let mut i = 0;
        let sizes = [7usize, 13, 3, 29, 1, 41];
        let mut k = 0;
        while i < bytes.len() {
            let n = sizes[k % sizes.len()].min(bytes.len() - i);
            chunks.push(Ok(Bytes::copy_from_slice(&bytes[i..i + n])));
            i += n;
            k += 1;
        }
        futures::stream::iter(chunks)
    }

    #[tokio::test]
    async fn assembles_streamed_text_and_tool_use() {
        let events: Vec<StreamEvent> = AnthropicProvider::decode_stream(chunked(include_str!(
            "../../tests/fixtures/anthropic_stream.sse"
        )))
        .map(|r| r.unwrap())
        .collect()
        .await;
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Let me check.");
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolUseStart { id, name } if id == "toolu_9" && name == "ls"
        )));
        let partial: String = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolInputDelta { partial_json } => Some(partial_json.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(partial, r#"{"path": "/etc"}"#);
        let StreamEvent::Done(resp) = events.last().unwrap() else {
            panic!("last event must be Done");
        };
        assert_eq!(resp.model, "claude-sonnet-5");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(
            resp.content,
            vec![
                ContentBlock::Text {
                    text: "Let me check.".into()
                },
                ContentBlock::ToolUse {
                    id: "toolu_9".into(),
                    name: "ls".into(),
                    input: json!({"path": "/etc"}),
                }
            ]
        );
        assert_eq!(resp.usage.input_tokens, 25);
        assert_eq!(resp.usage.output_tokens, 17);
        assert_eq!(resp.usage.cache_read_tokens, 400);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::Done(_)))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn truncated_stream_is_malformed() {
        let transcript = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"m\",\"usage\":{\"input_tokens\":1}}}\n\n";
        let events: Vec<_> =
            AnthropicProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(transcript),
            )]))
            .collect()
            .await;
        assert!(matches!(
            events.last(),
            Some(Err(ProviderError::Malformed(_)))
        ));
    }

    #[tokio::test]
    async fn stream_error_event_is_surfaced() {
        let transcript = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let events: Vec<_> =
            AnthropicProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(transcript),
            )]))
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Err(ProviderError::Other(m)) if m == "Overloaded"));
    }

    #[test]
    fn from_env_needs_a_credential() {
        // Only meaningful when the environment is clean; skip otherwise.
        if std::env::var_os("ANTHROPIC_API_KEY").is_some()
            || std::env::var_os("ANTHROPIC_AUTH_TOKEN").is_some()
        {
            return;
        }
        assert!(matches!(
            AnthropicProvider::from_env(),
            Err(ProviderError::Auth(_))
        ));
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let p = AnthropicProvider::new("k".into(), Some("https://proxy.example/".into()));
        assert_eq!(p.base_url(), "https://proxy.example");
        let d = AnthropicProvider::new("k".into(), None);
        assert_eq!(d.base_url(), DEFAULT_BASE_URL);
    }
}
