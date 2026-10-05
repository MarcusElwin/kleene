//! OpenAI-compatible chat completions adapter (`POST {base}/chat/completions`).
//!
//! This is the wire format OpenAI, Google, Mistral, Together, Fireworks,
//! Ollama, vLLM and most gateways expose, so one adapter covers them all;
//! only the base URL and the key differ.

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

/// Default API base (OpenAI itself).
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// The key a non-object output schema is wrapped under; see [`strict_schema`].
pub const WRAP_KEY: &str = "output";

/// The schema as OpenAI's strict structured outputs accept it, and whether
/// the model's answer arrives wrapped.
///
/// Strict mode wants the root to be an object with every property required
/// and `additionalProperties: false`; the same rewrite the Anthropic adapter
/// applies ([`super::anthropic::normalize_schema`]) supplies those. A root
/// that is not an object (`{"type": "array", ...}`, a bare string or number
/// schema) is wrapped as the single required property [`WRAP_KEY`] of an
/// object, and [`unwrap_output`] takes it back out of the reply, so the
/// caller sees the JSON shape it asked for.
pub fn strict_schema(schema: &Value) -> (Value, bool) {
    let normalized = super::anthropic::normalize_schema(schema);
    let is_object = normalized.get("type").and_then(Value::as_str) == Some("object");
    if is_object {
        (normalized, false)
    } else {
        (
            json!({
                "type": "object",
                "properties": { WRAP_KEY: normalized },
                "required": [WRAP_KEY],
                "additionalProperties": false
            }),
            true,
        )
    }
}

/// Undo [`strict_schema`]'s wrapping on a reply: the text content, parsed
/// as JSON, is replaced by its [`WRAP_KEY`] member. Text that is not an
/// object with that member is left alone.
pub fn unwrap_output(resp: &mut CompletionResponse) {
    let text = resp.text();
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(inner) = map.remove(WRAP_KEY) else {
        return;
    };
    resp.content
        .retain(|b| !matches!(b, ContentBlock::Text { .. }));
    resp.content.insert(
        0,
        ContentBlock::Text {
            text: inner.to_string(),
        },
    );
}

/// Chat-completions backend.
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    backoff: Vec<Duration>,
    /// The request field that carries the output token limit: see
    /// [`completion_limit_field`].
    limit_field: &'static str,
}

/// The name of the output token limit field for a server at `base_url`.
///
/// OpenAI's own endpoint (`api.openai.com`) rejects `max_tokens` for every
/// GPT-5 and GPT-6 model (`Unsupported parameter: 'max_tokens' is not
/// supported with this model. Use 'max_completion_tokens' instead.`), so
/// first-party requests send `max_completion_tokens`. Every other
/// compatible server (local servers, gateways, older proxies) still gets
/// `max_tokens`, which all of them accept.
pub fn completion_limit_field(base_url: &str) -> &'static str {
    let host = base_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("");
    if host == "api.openai.com" || host.ends_with(".api.openai.com") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "set"))
            .finish()
    }
}

impl OpenAiCompatProvider {
    /// Talk to `base_url` (the `/v1` root, e.g. `http://localhost:11434/v1`),
    /// with an optional bearer key. Local servers usually need none.
    pub fn new(api_key: Option<String>, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            api_key,
            limit_field: completion_limit_field(&base_url),
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff: DEFAULT_BACKOFF.to_vec(),
        }
    }

    /// Read `OPENAI_API_KEY` and `OPENAI_BASE_URL` (default
    /// [`DEFAULT_BASE_URL`]). Errors with [`ProviderError::Auth`] when neither
    /// is set: OpenAI itself needs a key, and a local server needs a URL.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = non_empty_env("OPENAI_API_KEY");
        let base = non_empty_env("OPENAI_BASE_URL");
        if key.is_none() && base.is_none() {
            return Err(ProviderError::Auth(
                "neither OPENAI_API_KEY nor OPENAI_BASE_URL is set".into(),
            ));
        }
        Ok(Self::new(
            key,
            base.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
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

    /// The JSON body sent for a request (without `stream`). Public so the
    /// mapping is testable and can be logged by the trace.
    ///
    /// `max_tokens` is sent as `max_tokens`, which every compatible server
    /// accepts, except to OpenAI itself, where the adapter sends
    /// `max_completion_tokens` (see [`completion_limit_field`]); a server
    /// that insists on another name can be given it through `options.extra`
    /// (extra keys override defaults).
    pub fn build_body(req: &CompletionRequest) -> Value {
        Self::build_body_with_limit(req, "max_tokens")
    }

    /// [`build_body`](Self::build_body) with the output token limit under
    /// `limit_field`.
    pub fn build_body_with_limit(req: &CompletionRequest, limit_field: &str) -> Value {
        let mut messages = Vec::new();
        if !req.system.is_empty() {
            messages.push(json!({"role": "system", "content": req.system}));
        }
        for m in &req.messages {
            messages.extend(message_to_wire(m));
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        body.insert("messages".into(), Value::Array(messages));
        body.insert(limit_field.to_string(), json!(req.max_tokens));
        if !req.tools.is_empty() {
            let tools = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(schema) = &req.output_schema {
            let (schema, _) = strict_schema(schema);
            body.insert(
                "response_format".into(),
                json!({
                    "type": "json_schema",
                    "json_schema": {"name": "output", "schema": schema, "strict": true}
                }),
            );
        }
        if let Some(effort) = &req.options.effort {
            body.insert("reasoning_effort".into(), json!(effort));
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
        let resp: ChatResponse =
            serde_json::from_str(body).map_err(|e| ProviderError::Malformed(e.to_string()))?;
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Malformed("response has no choices".into()))?;
        let mut content = Vec::new();
        let mut stop_reason = map_finish_reason(choice.finish_reason.as_deref());
        if let Some(refusal) = choice.message.refusal.filter(|r| !r.is_empty()) {
            content.push(ContentBlock::Text { text: refusal });
            stop_reason = StopReason::Refusal;
        }
        if let Some(text) = choice.message.content.and_then(content_text) {
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
        }
        for call in choice.message.tool_calls {
            content.push(call.into_block()?);
        }
        Ok(CompletionResponse {
            model: resp.model.unwrap_or_default(),
            content,
            stop_reason,
            usage: resp.usage.unwrap_or_default().into_usage(),
        })
    }

    /// Decode an SSE byte stream (the body of a `stream: true` request) into
    /// provider-neutral events, ending with [`StreamEvent::Done`] at
    /// `data: [DONE]`. Fixture transcripts and live responses take the same
    /// path.
    pub fn decode_stream<S, E>(bytes: S) -> BoxStream<'static, Result<StreamEvent, ProviderError>>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Display + Send + 'static,
    {
        sse::assemble(bytes, ChatAssembler::default())
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("content-type", "application/json");
        if let Some(k) = &self.api_key {
            r = r.bearer_auth(k);
        }
        r.json(body)
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// One of our messages becomes one or more wire messages: text and tool
/// calls stay on the speaker's message, each tool result becomes a `tool`
/// message. There is no wire equivalent of `is_error`; the result text is
/// passed through unchanged.
fn message_to_wire(m: &Message) -> Vec<Value> {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut out = Vec::new();
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let flush = |text: &mut String, tool_calls: &mut Vec<Value>, out: &mut Vec<Value>| {
        if text.is_empty() && tool_calls.is_empty() {
            return;
        }
        let mut msg = Map::new();
        msg.insert("role".into(), json!(role));
        msg.insert(
            "content".into(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(std::mem::take(text))
            },
        );
        if !tool_calls.is_empty() {
            msg.insert(
                "tool_calls".into(),
                Value::Array(std::mem::take(tool_calls)),
            );
        }
        out.push(Value::Object(msg));
    };
    for block in &m.content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": input.to_string()},
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                flush(&mut text, &mut tool_calls, &mut out);
                out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
            }
        }
    }
    flush(&mut text, &mut tool_calls, &mut out);
    out
}

fn map_finish_reason(s: Option<&str>) -> StopReason {
    match s {
        Some("stop") => StopReason::EndTurn,
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
        Some("content_filter") => StopReason::Refusal,
        _ => StopReason::Other,
    }
}

/// `content` is a string on almost every server, but some return an array
/// of `{type: "text", text}` parts.
fn content_text(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    model: Option<String>,
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChatMessage {
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct ToolCall {
    id: String,
    function: FunctionCall,
}

#[derive(Deserialize)]
struct FunctionCall {
    name: String,
    #[serde(default)]
    arguments: String,
}

impl ToolCall {
    fn into_block(self) -> Result<ContentBlock, ProviderError> {
        Ok(ContentBlock::ToolUse {
            id: self.id,
            name: self.function.name.clone(),
            input: parse_arguments(&self.function.name, &self.function.arguments)?,
        })
    }
}

fn parse_arguments(name: &str, arguments: &str) -> Result<Value, ProviderError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments)
        .map_err(|e| ProviderError::Malformed(format!("tool arguments for {name}: {e}")))
}

#[derive(Deserialize, Default, Clone, Copy)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct PromptTokensDetails {
    cached_tokens: Option<u64>,
}

impl ChatUsage {
    fn into_usage(self) -> Usage {
        let cached = self
            .prompt_tokens_details
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        Usage {
            input_tokens: self.prompt_tokens.saturating_sub(cached),
            output_tokens: self.completion_tokens,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
            cost_usd: None,
        }
    }
}

/// A streamed tool call, keyed by its `index`.
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Assembles `data:` chunks into a response; `[DONE]` finishes it.
#[derive(Default)]
struct ChatAssembler {
    started: bool,
    model: String,
    text: String,
    refusal: String,
    calls: Vec<PartialCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

impl ChatAssembler {
    fn finish_response(&mut self) -> Result<CompletionResponse, ProviderError> {
        let mut content = Vec::new();
        let mut stop_reason = map_finish_reason(self.finish_reason.as_deref());
        if !self.refusal.is_empty() {
            content.push(ContentBlock::Text {
                text: std::mem::take(&mut self.refusal),
            });
            stop_reason = StopReason::Refusal;
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text {
                text: std::mem::take(&mut self.text),
            });
        }
        for c in self.calls.drain(..) {
            content.push(ContentBlock::ToolUse {
                input: parse_arguments(&c.name, &c.arguments)?,
                id: c.id,
                name: c.name,
            });
        }
        Ok(CompletionResponse {
            model: std::mem::take(&mut self.model),
            content,
            stop_reason,
            usage: self.usage.take().unwrap_or_default(),
        })
    }
}

impl Assembler for ChatAssembler {
    fn on_event(&mut self, ev: SseEvent) -> Result<Vec<StreamEvent>, ProviderError> {
        if ev.data.trim() == "[DONE]" {
            return Ok(vec![StreamEvent::Done(self.finish_response()?)]);
        }
        let chunk: Value = serde_json::from_str(&ev.data)
            .map_err(|e| ProviderError::Malformed(format!("stream chunk: {e}")))?;
        if let Some(err) = chunk.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown stream error");
            return Err(ProviderError::Other(msg.to_string()));
        }
        self.started = true;
        if let Some(m) = chunk.get("model").and_then(Value::as_str) {
            if self.model.is_empty() {
                self.model = m.to_string();
            }
        }
        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            let u: ChatUsage = serde_json::from_value(u.clone()).unwrap_or_default();
            self.usage = Some(u.into_usage());
        }
        let mut out = Vec::new();
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(out);
        };
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(fr.to_string());
        }
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        if let Some(text) = delta.get("content").cloned().and_then(content_text) {
            if !text.is_empty() {
                self.text.push_str(&text);
                out.push(StreamEvent::TextDelta { text });
            }
        }
        if let Some(r) = delta.get("refusal").and_then(Value::as_str) {
            self.refusal.push_str(r);
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while self.calls.len() <= index {
                    self.calls.push(PartialCall::default());
                }
                let slot = &mut self.calls[index];
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    slot.id = id.to_string();
                }
                let mut started = false;
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    if slot.name.is_empty() {
                        started = true;
                    }
                    slot.name = name.to_string();
                }
                if started {
                    out.push(StreamEvent::ToolUseStart {
                        id: slot.id.clone(),
                        name: slot.name.clone(),
                    });
                }
                if let Some(args) = call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                {
                    slot.arguments.push_str(args);
                    out.push(StreamEvent::ToolInputDelta {
                        partial_json: args.to_string(),
                    });
                }
            }
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        if self.started {
            // Some servers close the connection without `[DONE]`; the
            // response is still complete if a finish reason arrived.
            if self.finish_reason.is_some() {
                return Ok(vec![StreamEvent::Done(self.finish_response()?)]);
            }
            Err(ProviderError::Malformed(
                "stream ended before [DONE] or a finish_reason".into(),
            ))
        } else {
            Err(ProviderError::Malformed("empty stream".into()))
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        "openai_compat"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            json_schema: true,
            // Caching is automatic and only visible once usage reports it.
            prompt_cache: false,
            reasoning_control: true,
            cost_reported: false,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let wrapped = req
            .output_schema
            .as_ref()
            .is_some_and(|s| strict_schema(s).1);
        let body = Self::build_body_with_limit(&req, self.limit_field);
        let resp = http::send(&self.backoff, || self.request(&body)).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(http::describe(&e)))?;
        let mut parsed = Self::parse_response(&text)?;
        if wrapped {
            unwrap_output(&mut parsed);
        }
        Ok(parsed)
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        if req
            .output_schema
            .as_ref()
            .is_some_and(|s| strict_schema(s).1)
        {
            // A wrapped schema's reply has to be unwrapped whole, so the
            // call is made unstreamed and replayed as one delta.
            let resp = self.complete(req).await?;
            return Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::TextDelta { text: resp.text() }),
                Ok(StreamEvent::Done(resp)),
            ])));
        }
        let mut body = Self::build_body_with_limit(&req, self.limit_field);
        body["stream"] = json!(true);
        body["stream_options"] = json!({"include_usage": true});
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
            model: "gpt-5.4-mini".into(),
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
                            id: "call_1".into(),
                            name: "ls".into(),
                            input: json!({"path": "."}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        ContentBlock::ToolResult {
                            tool_use_id: "call_1".into(),
                            content: "a.txt".into(),
                            is_error: false,
                        },
                        ContentBlock::Text {
                            text: "and now?".into(),
                        },
                    ],
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
            max_tokens: 256,
            options: ProviderOptions {
                effort: Some("low".into()),
                cache_prefix: true,
                temperature: Some(0.0),
                extra: [("seed".to_string(), json!(7))].into_iter().collect(),
            },
        }
    }

    #[test]
    fn openai_itself_gets_max_completion_tokens() {
        assert_eq!(
            completion_limit_field("https://api.openai.com/v1"),
            "max_completion_tokens"
        );
        assert_eq!(
            completion_limit_field("http://localhost:11434/v1"),
            "max_tokens"
        );
        assert_eq!(
            completion_limit_field("https://gateway.example.com/openai/v1"),
            "max_tokens"
        );
        let p = OpenAiCompatProvider::new(Some("k".into()), DEFAULT_BASE_URL.into());
        let body = OpenAiCompatProvider::build_body_with_limit(&req(), p.limit_field);
        assert_eq!(body["max_completion_tokens"], 256);
        assert!(body.get("max_tokens").is_none());
        let local = OpenAiCompatProvider::new(None, "http://localhost:11434/v1".into());
        assert_eq!(local.limit_field, "max_tokens");
    }

    #[test]
    fn non_object_schemas_are_wrapped_and_unwrapped() {
        let (array, wrapped) =
            strict_schema(&json!({"type": "array", "items": {"type": "string"}}));
        assert!(wrapped);
        assert_eq!(array["type"], "object");
        assert_eq!(array["required"], json!(["output"]));
        assert_eq!(array["additionalProperties"], false);
        assert_eq!(array["properties"]["output"]["type"], "array");

        let (object, wrapped) =
            strict_schema(&json!({"type": "object", "properties": {"ok": {"type": "boolean"}}}));
        assert!(!wrapped);
        assert_eq!(object["required"], json!(["ok"]));
        assert_eq!(object["additionalProperties"], false);

        let mut r = req();
        r.output_schema = Some(json!({"type": "array", "items": {"type": "integer"}}));
        let body = OpenAiCompatProvider::build_body(&r);
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );

        let mut resp = CompletionResponse {
            model: "m".into(),
            content: vec![ContentBlock::Text {
                text: r#"{"output":[1,2,3]}"#.into(),
            }],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        unwrap_output(&mut resp);
        assert_eq!(resp.text(), "[1,2,3]");

        let mut plain = resp.clone();
        plain.content = vec![ContentBlock::Text {
            text: r#"{"ok":true}"#.into(),
        }];
        unwrap_output(&mut plain);
        assert_eq!(plain.text(), r#"{"ok":true}"#);
    }

    #[test]
    fn body_maps_every_field() {
        let body = OpenAiCompatProvider::build_body(&req());
        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["max_tokens"], 256);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "You are terse."},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "let me look", "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "ls", "arguments": "{\"path\":\".\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "a.txt"},
                {"role": "user", "content": "and now?"}
            ])
        );
        assert_eq!(
            body["tools"],
            json!([{"type": "function", "function": {"name": "ls", "description": "list",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}}])
        );
        assert_eq!(
            body["response_format"],
            json!({"type": "json_schema", "json_schema": {
                "name": "output", "strict": true,
                "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}},
                           "required": ["ok"], "additionalProperties": false}}})
        );
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["temperature"], json!(0.0));
        assert_eq!(body["seed"], 7);
        assert!(body.get("stream").is_none());
        assert!(body.get("cache_control").is_none());
    }

    #[test]
    fn body_omits_optional_parts() {
        let mut r = req();
        r.system.clear();
        r.tools.clear();
        r.output_schema = None;
        r.options = ProviderOptions::default();
        let body = OpenAiCompatProvider::build_body(&r);
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("tools").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn assistant_tool_call_only_has_null_content() {
        let m = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "c".into(),
                name: "f".into(),
                input: json!({}),
            }],
        };
        let wire = message_to_wire(&m);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["content"], Value::Null);
        assert_eq!(wire[0]["tool_calls"][0]["function"]["arguments"], "{}");
    }

    #[test]
    fn parses_text_response_with_cached_tokens() {
        let resp = OpenAiCompatProvider::parse_response(include_str!(
            "../../tests/fixtures/openai_text.json"
        ))
        .unwrap();
        assert_eq!(resp.model, "gpt-5.4-mini");
        assert_eq!(resp.text(), "Hello there.");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 20,
                output_tokens: 5,
                cache_read_tokens: 80,
                cache_write_tokens: 0,
                cost_usd: None,
            }
        );
    }

    #[test]
    fn parses_tool_calls_response() {
        let resp = OpenAiCompatProvider::parse_response(include_str!(
            "../../tests/fixtures/openai_tool_calls.json"
        ))
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(
            resp.content,
            vec![ContentBlock::ToolUse {
                id: "call_abc".into(),
                name: "ls".into(),
                input: json!({"path": "/tmp"}),
            }]
        );
    }

    #[test]
    fn refusal_field_maps_to_refusal_stop() {
        let body = r#"{"model":"m","choices":[{"message":{"role":"assistant","content":null,"refusal":"I can't help with that."},"finish_reason":"stop"}]}"#;
        let resp = OpenAiCompatProvider::parse_response(body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::Refusal);
        assert_eq!(resp.text(), "I can't help with that.");
    }

    #[test]
    fn content_filter_and_length_map() {
        for (fr, want) in [
            ("length", StopReason::MaxTokens),
            ("content_filter", StopReason::Refusal),
            ("weird", StopReason::Other),
        ] {
            let body = format!(
                r#"{{"model":"m","choices":[{{"message":{{"content":"x"}},"finish_reason":"{fr}"}}]}}"#
            );
            assert_eq!(
                OpenAiCompatProvider::parse_response(&body)
                    .unwrap()
                    .stop_reason,
                want
            );
        }
    }

    #[test]
    fn no_choices_is_malformed() {
        assert!(matches!(
            OpenAiCompatProvider::parse_response(r#"{"choices":[]}"#),
            Err(ProviderError::Malformed(_))
        ));
        assert!(matches!(
            OpenAiCompatProvider::parse_response("nope"),
            Err(ProviderError::Malformed(_))
        ));
    }

    fn chunked(transcript: &'static str) -> impl Stream<Item = Result<Bytes, Infallible>> {
        let bytes = transcript.as_bytes();
        let mut chunks = Vec::new();
        let mut i = 0;
        let sizes = [11usize, 5, 37, 2, 19];
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
    async fn assembles_streamed_text_and_tool_calls() {
        let events: Vec<StreamEvent> = OpenAiCompatProvider::decode_stream(chunked(include_str!(
            "../../tests/fixtures/openai_stream.sse"
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
        assert_eq!(text, "Sure, checking.");
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolUseStart { id, name } if id == "call_7" && name == "ls"
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
        assert_eq!(resp.model, "gpt-5.4-mini");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(
            resp.content,
            vec![
                ContentBlock::Text {
                    text: "Sure, checking.".into()
                },
                ContentBlock::ToolUse {
                    id: "call_7".into(),
                    name: "ls".into(),
                    input: json!({"path": "/etc"}),
                }
            ]
        );
        assert_eq!(resp.usage.input_tokens, 30);
        assert_eq!(resp.usage.cache_read_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 12);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::Done(_)))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn stream_without_done_but_with_finish_reason_completes() {
        let transcript = "data: {\"model\":\"m\",\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n";
        let events: Vec<_> =
            OpenAiCompatProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(transcript),
            )]))
            .collect()
            .await;
        let StreamEvent::Done(resp) = events.last().unwrap().as_ref().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(resp.text(), "ok");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn empty_stream_is_malformed() {
        let events: Vec<_> =
            OpenAiCompatProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from_static(b""),
            )]))
            .collect()
            .await;
        assert!(matches!(
            events.last(),
            Some(Err(ProviderError::Malformed(_)))
        ));
    }

    #[test]
    fn from_env_needs_key_or_url() {
        if std::env::var_os("OPENAI_API_KEY").is_some()
            || std::env::var_os("OPENAI_BASE_URL").is_some()
        {
            return;
        }
        assert!(matches!(
            OpenAiCompatProvider::from_env(),
            Err(ProviderError::Auth(_))
        ));
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let p = OpenAiCompatProvider::new(None, "http://localhost:11434/v1/".into());
        assert_eq!(p.base_url(), "http://localhost:11434/v1");
    }
}
