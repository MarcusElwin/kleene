//! Open Responses gateway adapter (`POST {base}/responses`), behind the
//! `gateway` feature.
//!
//! This is the Responses API shape that Aura and other Open Responses
//! gateways (and OpenAI itself) expose: one `input` list of items rather
//! than a chat transcript, `instructions` for the system prefix, semantic
//! SSE events (`response.output_text.delta`, `response.completed`, …) and,
//! from a gateway, `usage.cost_usd`, which is why this is the one adapter
//! whose [`Capabilities::cost_reported`] is true.

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

/// Default API base (OpenAI's own Responses endpoint lives under `/v1`).
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Open Responses backend.
pub struct OpenResponsesProvider {
    client: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
    backoff: Vec<Duration>,
}

impl std::fmt::Debug for OpenResponsesProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenResponsesProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "set"))
            .finish()
    }
}

impl OpenResponsesProvider {
    /// Talk to `base_url` (the root under which `/responses` lives, e.g.
    /// `https://aura.example/v1`), with an optional bearer key.
    pub fn new(api_key: Option<String>, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            backoff: DEFAULT_BACKOFF.to_vec(),
        }
    }

    /// Read `OPEN_RESPONSES_API_KEY` and `OPEN_RESPONSES_BASE_URL` (default
    /// [`DEFAULT_BASE_URL`]). Errors with [`ProviderError::Auth`] when neither
    /// is set: a gateway needs at least a URL.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = non_empty_env("OPEN_RESPONSES_API_KEY");
        let base = non_empty_env("OPEN_RESPONSES_BASE_URL");
        if key.is_none() && base.is_none() {
            return Err(ProviderError::Auth(
                "neither OPEN_RESPONSES_API_KEY nor OPEN_RESPONSES_BASE_URL is set".into(),
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
    /// The system prefix goes in `instructions`; each message becomes a
    /// `message` item, each tool call a `function_call` item and each tool
    /// result a `function_call_output` item, in transcript order.
    pub fn build_body(req: &CompletionRequest) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(req.model));
        if !req.system.is_empty() {
            body.insert("instructions".into(), json!(req.system));
        }
        let mut input = Vec::new();
        for m in &req.messages {
            input.extend(message_to_wire(m));
        }
        body.insert("input".into(), Value::Array(input));
        body.insert("max_output_tokens".into(), json!(req.max_tokens));
        if !req.tools.is_empty() {
            let tools = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(schema) = &req.output_schema {
            body.insert(
                "text".into(),
                json!({
                    "format": {
                        "type": "json_schema",
                        "name": "output",
                        "schema": schema,
                        "strict": true
                    }
                }),
            );
        }
        if let Some(effort) = &req.options.effort {
            body.insert("reasoning".into(), json!({"effort": effort}));
        }
        if let Some(t) = req.options.temperature {
            body.insert("temperature".into(), super::f32_json(t));
        }
        for (k, v) in &req.options.extra {
            body.insert(k.clone(), v.clone());
        }
        Value::Object(body)
    }

    /// Parse a non-streaming response body (a `response` object).
    pub fn parse_response(body: &str) -> Result<CompletionResponse, ProviderError> {
        let resp: ApiResponse =
            serde_json::from_str(body).map_err(|e| ProviderError::Malformed(e.to_string()))?;
        resp.into_completion()
    }

    /// Decode an SSE byte stream (the body of a `stream: true` request) into
    /// provider-neutral events, ending with [`StreamEvent::Done`] at
    /// `response.completed`. Fixture transcripts and live responses take the
    /// same path.
    pub fn decode_stream<S, E>(bytes: S) -> BoxStream<'static, Result<StreamEvent, ProviderError>>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Display + Send + 'static,
    {
        sse::assemble(bytes, ResponseAssembler::default())
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .post(format!("{}/responses", self.base_url))
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

/// One of our messages becomes one or more input items: consecutive text
/// blocks form one `message` item (user text is `input_text`, assistant
/// text is `output_text`), each tool call a `function_call` and each tool
/// result a `function_call_output`. There is no wire equivalent of
/// `is_error`; the result text is passed through unchanged.
fn message_to_wire(m: &Message) -> Vec<Value> {
    let (role, part_type) = match m.role {
        Role::User => ("user", "input_text"),
        Role::Assistant => ("assistant", "output_text"),
    };
    let mut out = Vec::new();
    let mut parts: Vec<Value> = Vec::new();
    let flush = |parts: &mut Vec<Value>, out: &mut Vec<Value>| {
        if parts.is_empty() {
            return;
        }
        out.push(json!({
            "type": "message",
            "role": role,
            "content": Value::Array(std::mem::take(parts)),
        }));
    };
    for block in &m.content {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"type": part_type, "text": text})),
            ContentBlock::ToolUse { id, name, input } => {
                flush(&mut parts, &mut out);
                out.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": input.to_string(),
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                flush(&mut parts, &mut out);
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": content,
                }));
            }
        }
    }
    flush(&mut parts, &mut out);
    out
}

fn parse_arguments(name: &str, arguments: &str) -> Result<Value, ProviderError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments)
        .map_err(|e| ProviderError::Malformed(format!("tool arguments for {name}: {e}")))
}

/// A `response` object, as returned whole or carried by `response.completed`.
#[derive(Deserialize, Default)]
struct ApiResponse {
    #[serde(default)]
    model: String,
    status: Option<String>,
    incomplete_details: Option<IncompleteDetails>,
    error: Option<ApiError>,
    #[serde(default)]
    output: Vec<ApiItem>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize, Default)]
struct IncompleteDetails {
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ApiError {
    message: Option<String>,
}

/// One output item.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiItem {
    Message {
        #[serde(default)]
        content: Vec<ApiPart>,
    },
    FunctionCall {
        #[serde(default)]
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: String,
    },
    #[serde(other)]
    Other,
}

/// One part of a `message` item's content.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiPart {
    OutputText {
        text: String,
    },
    Refusal {
        refusal: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    input_tokens_details: Option<InputTokensDetails>,
    /// Added by gateways that price the call.
    cost_usd: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct InputTokensDetails {
    cached_tokens: Option<u64>,
}

impl ApiUsage {
    fn into_usage(self) -> Usage {
        let cached = self
            .input_tokens_details
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0);
        Usage {
            input_tokens: self.input_tokens.saturating_sub(cached),
            output_tokens: self.output_tokens,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
            cost_usd: self.cost_usd,
        }
    }
}

/// Content blocks out of output items, plus whether a refusal was among
/// them (a refusal is a text block and a stop reason, not an error).
fn items_to_content(items: Vec<ApiItem>) -> Result<(Vec<ContentBlock>, bool), ProviderError> {
    let mut content = Vec::new();
    let mut refused = false;
    for item in items {
        match item {
            ApiItem::Message { content: parts } => {
                for p in parts {
                    match p {
                        ApiPart::OutputText { text } if !text.is_empty() => {
                            content.push(ContentBlock::Text { text });
                        }
                        ApiPart::Refusal { refusal } => {
                            refused = true;
                            if !refusal.is_empty() {
                                content.push(ContentBlock::Text { text: refusal });
                            }
                        }
                        _ => {}
                    }
                }
            }
            ApiItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => content.push(ContentBlock::ToolUse {
                input: parse_arguments(&name, &arguments)?,
                id: call_id,
                name,
            }),
            ApiItem::Other => {}
        }
    }
    Ok((content, refused))
}

fn map_stop_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
    content: &[ContentBlock],
    refused: bool,
) -> StopReason {
    if refused {
        return StopReason::Refusal;
    }
    if content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    {
        return StopReason::ToolUse;
    }
    match status {
        Some("completed") => StopReason::EndTurn,
        Some("incomplete") => match incomplete_reason {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            _ => StopReason::Other,
        },
        _ => StopReason::Other,
    }
}

impl ApiResponse {
    fn into_completion(self) -> Result<CompletionResponse, ProviderError> {
        if self.status.as_deref() == Some("failed") {
            let msg = self
                .error
                .and_then(|e| e.message)
                .unwrap_or_else(|| "response failed".into());
            return Err(ProviderError::Other(msg));
        }
        let (content, refused) = items_to_content(self.output)?;
        let stop_reason = map_stop_reason(
            self.status.as_deref(),
            self.incomplete_details.and_then(|d| d.reason).as_deref(),
            &content,
            refused,
        );
        Ok(CompletionResponse {
            model: self.model,
            content,
            stop_reason,
            usage: self.usage.unwrap_or_default().into_usage(),
        })
    }
}

/// An output item under construction while streaming, keyed by
/// `output_index`.
enum PartialItem {
    Message {
        text: String,
        refusal: String,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Other,
}

/// Assembles `response.*` events into a response; `response.completed`
/// (or `response.incomplete`) finishes it.
#[derive(Default)]
struct ResponseAssembler {
    started: bool,
    model: String,
    items: Vec<PartialItem>,
}

impl ResponseAssembler {
    fn slot(&mut self, index: usize) -> &mut PartialItem {
        while self.items.len() <= index {
            self.items.push(PartialItem::Other);
        }
        &mut self.items[index]
    }

    /// The response assembled from deltas, used when the terminal event
    /// carries no `output` of its own.
    fn assembled(&mut self) -> Result<(Vec<ContentBlock>, bool), ProviderError> {
        let mut content = Vec::new();
        let mut refused = false;
        for item in self.items.drain(..) {
            match item {
                PartialItem::Message { text, refusal } => {
                    if !refusal.is_empty() {
                        refused = true;
                        content.push(ContentBlock::Text { text: refusal });
                    }
                    if !text.is_empty() {
                        content.push(ContentBlock::Text { text });
                    }
                }
                PartialItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => content.push(ContentBlock::ToolUse {
                    input: parse_arguments(&name, &arguments)?,
                    id: call_id,
                    name,
                }),
                PartialItem::Other => {}
            }
        }
        Ok((content, refused))
    }

    fn finish_response(&mut self, data: &Value) -> Result<CompletionResponse, ProviderError> {
        let mut resp: ApiResponse = data
            .get("response")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| ProviderError::Malformed(format!("final response: {e}")))?
            .unwrap_or_default();
        if resp.model.is_empty() {
            resp.model = std::mem::take(&mut self.model);
        }
        if resp.output.is_empty() {
            let (content, refused) = self.assembled()?;
            let stop_reason = map_stop_reason(
                resp.status.as_deref(),
                resp.incomplete_details.and_then(|d| d.reason).as_deref(),
                &content,
                refused,
            );
            return Ok(CompletionResponse {
                model: resp.model,
                content,
                stop_reason,
                usage: resp.usage.unwrap_or_default().into_usage(),
            });
        }
        resp.into_completion()
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Assembler for ResponseAssembler {
    fn on_event(&mut self, ev: SseEvent) -> Result<Vec<StreamEvent>, ProviderError> {
        let data: Value = serde_json::from_str(&ev.data)
            .map_err(|e| ProviderError::Malformed(format!("stream event: {e}")))?;
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(ev.event)
            .unwrap_or_default();
        let index = data
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let mut out = Vec::new();
        match kind.as_str() {
            "response.created" | "response.in_progress" => {
                self.started = true;
                if let Some(m) = data.pointer("/response/model").and_then(Value::as_str) {
                    self.model = m.to_string();
                }
            }
            "response.output_item.added" => {
                self.started = true;
                let item = data.get("item").cloned().unwrap_or(Value::Null);
                let partial = match item.get("type").and_then(Value::as_str) {
                    Some("message") => PartialItem::Message {
                        text: String::new(),
                        refusal: String::new(),
                    },
                    Some("function_call") => {
                        let call_id = str_field(&item, "call_id");
                        let name = str_field(&item, "name");
                        out.push(StreamEvent::ToolUseStart {
                            id: call_id.clone(),
                            name: name.clone(),
                        });
                        PartialItem::FunctionCall {
                            call_id,
                            name,
                            arguments: str_field(&item, "arguments"),
                        }
                    }
                    _ => PartialItem::Other,
                };
                *self.slot(index) = partial;
            }
            "response.output_text.delta" => {
                self.started = true;
                let delta = str_field(&data, "delta");
                if delta.is_empty() {
                    return Ok(out);
                }
                match self.slot(index) {
                    PartialItem::Message { text, .. } => text.push_str(&delta),
                    slot => {
                        *slot = PartialItem::Message {
                            text: delta.clone(),
                            refusal: String::new(),
                        }
                    }
                }
                out.push(StreamEvent::TextDelta { text: delta });
            }
            "response.refusal.delta" => {
                let delta = str_field(&data, "delta");
                match self.slot(index) {
                    PartialItem::Message { refusal, .. } => refusal.push_str(&delta),
                    slot => {
                        *slot = PartialItem::Message {
                            text: String::new(),
                            refusal: delta,
                        }
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = str_field(&data, "delta");
                if delta.is_empty() {
                    return Ok(out);
                }
                if let PartialItem::FunctionCall { arguments, .. } = self.slot(index) {
                    arguments.push_str(&delta);
                }
                out.push(StreamEvent::ToolInputDelta {
                    partial_json: delta,
                });
            }
            "response.function_call_arguments.done" => {
                // The full argument string is authoritative.
                if let Some(full) = data.get("arguments").and_then(Value::as_str) {
                    if let PartialItem::FunctionCall { arguments, .. } = self.slot(index) {
                        *arguments = full.to_string();
                    }
                }
            }
            "response.output_item.done" => {
                // The finished item replaces whatever the deltas built.
                let item = data.get("item").cloned().unwrap_or(Value::Null);
                if let Ok(parsed) = serde_json::from_value::<ApiItem>(item) {
                    let partial = match parsed {
                        ApiItem::Message { content } => {
                            let mut text = String::new();
                            let mut refusal = String::new();
                            for p in content {
                                match p {
                                    ApiPart::OutputText { text: t } => text.push_str(&t),
                                    ApiPart::Refusal { refusal: r } => refusal.push_str(&r),
                                    ApiPart::Other => {}
                                }
                            }
                            PartialItem::Message { text, refusal }
                        }
                        ApiItem::FunctionCall {
                            call_id,
                            name,
                            arguments,
                        } => PartialItem::FunctionCall {
                            call_id,
                            name,
                            arguments,
                        },
                        ApiItem::Other => PartialItem::Other,
                    };
                    *self.slot(index) = partial;
                }
            }
            "response.completed" | "response.incomplete" => {
                out.push(StreamEvent::Done(self.finish_response(&data)?));
            }
            "response.failed" => {
                let msg = data
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("response failed");
                return Err(ProviderError::Other(msg.to_string()));
            }
            "error" => {
                let msg = data
                    .pointer("/error/message")
                    .or_else(|| data.get("message"))
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
                "stream ended before response.completed".into(),
            ))
        } else {
            Err(ProviderError::Malformed("empty stream".into()))
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenResponsesProvider {
    fn name(&self) -> &str {
        "open_responses"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            json_schema: true,
            // Caching is the gateway's business and only visible in usage.
            prompt_cache: false,
            reasoning_control: true,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let body = Self::build_body(&req);
        let resp = http::send(&self.backoff, || self.request(&body)).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(http::describe(&e)))?;
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
                extra: [("metadata".to_string(), json!({"goal": "cheap"}))]
                    .into_iter()
                    .collect(),
            },
        }
    }

    #[test]
    fn body_maps_every_field() {
        let body = OpenResponsesProvider::build_body(&req());
        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["instructions"], "You are terse.");
        assert_eq!(body["max_output_tokens"], 256);
        assert_eq!(
            body["input"],
            json!([
                {"type": "message", "role": "user",
                 "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "let me look"}]},
                {"type": "function_call", "call_id": "call_1", "name": "ls",
                 "arguments": "{\"path\":\".\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "a.txt"},
                {"type": "message", "role": "user",
                 "content": [{"type": "input_text", "text": "and now?"}]}
            ])
        );
        assert_eq!(
            body["tools"],
            json!([{"type": "function", "name": "ls", "description": "list",
                    "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}])
        );
        assert_eq!(
            body["text"],
            json!({"format": {
                "type": "json_schema", "name": "output", "strict": true,
                "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}}}}})
        );
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
        assert_eq!(body["temperature"], json!(0.0));
        assert_eq!(body["metadata"], json!({"goal": "cheap"}));
        assert!(body.get("stream").is_none());
        assert!(body.get("messages").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn body_omits_optional_parts() {
        let mut r = req();
        r.system.clear();
        r.tools.clear();
        r.output_schema = None;
        r.options = ProviderOptions::default();
        let body = OpenResponsesProvider::build_body(&r);
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("text").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("temperature").is_none());
        assert_eq!(body["input"][0]["role"], "user");
    }

    #[test]
    fn assistant_tool_call_only_is_one_function_call_item() {
        let m = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "c".into(),
                name: "f".into(),
                input: json!({}),
            }],
        };
        let wire = message_to_wire(&m);
        assert_eq!(
            wire,
            vec![json!({"type": "function_call", "call_id": "c", "name": "f", "arguments": "{}"})]
        );
    }

    #[test]
    fn parses_text_response_with_cached_tokens_and_cost() {
        let resp = OpenResponsesProvider::parse_response(include_str!(
            "../../tests/fixtures/open_responses_text.json"
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
                cost_usd: Some(0.000125),
            }
        );
    }

    #[test]
    fn parses_tool_call_response_and_skips_reasoning_items() {
        let resp = OpenResponsesProvider::parse_response(include_str!(
            "../../tests/fixtures/open_responses_tool_call.json"
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
                    id: "call_abc".into(),
                    name: "ls".into(),
                    input: json!({"path": "/tmp"}),
                }
            ]
        );
        assert_eq!(resp.usage.cost_usd, None, "no gateway pricing");
    }

    #[test]
    fn refusal_is_a_stop_reason_not_an_error() {
        let resp = OpenResponsesProvider::parse_response(include_str!(
            "../../tests/fixtures/open_responses_refusal.json"
        ))
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::Refusal);
        assert_eq!(resp.text(), "I can't help with that.");
    }

    #[test]
    fn incomplete_statuses_map() {
        for (reason, want) in [
            ("max_output_tokens", StopReason::MaxTokens),
            ("content_filter", StopReason::Refusal),
            ("weird", StopReason::Other),
        ] {
            let body = format!(
                r#"{{"model":"m","status":"incomplete","incomplete_details":{{"reason":"{reason}"}},
                    "output":[{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"x"}}]}}]}}"#
            );
            assert_eq!(
                OpenResponsesProvider::parse_response(&body)
                    .unwrap()
                    .stop_reason,
                want
            );
        }
    }

    #[test]
    fn failed_status_is_an_error() {
        let body = r#"{"model":"m","status":"failed","error":{"code":"server_error","message":"boom"},"output":[]}"#;
        assert!(matches!(
            OpenResponsesProvider::parse_response(body),
            Err(ProviderError::Other(m)) if m == "boom"
        ));
    }

    #[test]
    fn malformed_body_is_reported() {
        assert!(matches!(
            OpenResponsesProvider::parse_response("nope"),
            Err(ProviderError::Malformed(_))
        ));
        let body = r#"{"model":"m","status":"completed","output":[{"type":"function_call","call_id":"c","name":"f","arguments":"{not json"}]}"#;
        assert!(matches!(
            OpenResponsesProvider::parse_response(body),
            Err(ProviderError::Malformed(m)) if m.contains("tool arguments for f")
        ));
    }

    fn chunked(transcript: &'static str) -> impl Stream<Item = Result<Bytes, Infallible>> {
        let bytes = transcript.as_bytes();
        let mut chunks = Vec::new();
        let mut i = 0;
        let sizes = [9usize, 23, 4, 31, 1, 17];
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
    async fn assembles_streamed_text_and_function_call() {
        let events: Vec<StreamEvent> = OpenResponsesProvider::decode_stream(chunked(include_str!(
            "../../tests/fixtures/open_responses_stream.sse"
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
        assert_eq!(
            resp.usage,
            Usage {
                input_tokens: 30,
                output_tokens: 12,
                cache_read_tokens: 10,
                cache_write_tokens: 0,
                cost_usd: Some(0.00031),
            }
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::Done(_)))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn completed_without_output_uses_the_assembled_deltas() {
        let transcript = concat!(
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"ok\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"m\",\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
        );
        let events: Vec<_> =
            OpenResponsesProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(transcript),
            )]))
            .collect()
            .await;
        let StreamEvent::Done(resp) = events.last().unwrap().as_ref().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(resp.model, "m");
        assert_eq!(resp.text(), "ok");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 3);
    }

    #[tokio::test]
    async fn truncated_stream_is_malformed() {
        let transcript = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"model\":\"m\"}}\n\n";
        let events: Vec<_> =
            OpenResponsesProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
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
    async fn failed_and_error_events_are_surfaced() {
        let failed = "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"Upstream down\"}}}\n\n";
        let events: Vec<_> =
            OpenResponsesProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(failed),
            )]))
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Err(ProviderError::Other(m)) if m == "Upstream down"));

        let error = "event: error\ndata: {\"type\":\"error\",\"code\":\"rate_limit\",\"message\":\"Slow down\"}\n\n";
        let events: Vec<_> =
            OpenResponsesProvider::decode_stream(futures::stream::iter(vec![Ok::<_, Infallible>(
                Bytes::from(error),
            )]))
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Err(ProviderError::Other(m)) if m == "Slow down"));
    }

    #[test]
    fn from_env_needs_key_or_url() {
        if std::env::var_os("OPEN_RESPONSES_API_KEY").is_some()
            || std::env::var_os("OPEN_RESPONSES_BASE_URL").is_some()
        {
            return;
        }
        assert!(matches!(
            OpenResponsesProvider::from_env(),
            Err(ProviderError::Auth(_))
        ));
    }

    #[test]
    fn capabilities_report_cost() {
        let p = OpenResponsesProvider::new(None, "https://aura.example/v1/".into());
        assert_eq!(p.base_url(), "https://aura.example/v1");
        assert_eq!(p.name(), "open_responses");
        assert!(p.capabilities().cost_reported);
    }
}
