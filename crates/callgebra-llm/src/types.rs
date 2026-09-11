//! Request and response types shared by every provider.

use callgebra_core::ModelAlias;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Who said what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The harness or the user.
    User,
    /// The model.
    Assistant,
}

/// One block of message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// A tool call requested by the model.
    ToolUse {
        /// Provider-assigned id.
        id: String,
        /// Tool name.
        name: String,
        /// Arguments as JSON.
        input: serde_json::Value,
    },
    /// The result of a tool call, sent back by the harness.
    ToolResult {
        /// Id of the `ToolUse` this answers.
        tool_use_id: String,
        /// Result text.
        content: String,
        /// The tool failed.
        is_error: bool,
    },
}

/// One message in the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Speaker.
    pub role: Role,
    /// Content blocks.
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// A single-text user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
    /// A single-text assistant message.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    /// Name.
    pub name: String,
    /// Description shown to the model.
    pub description: String,
    /// JSON schema of the arguments.
    pub input_schema: serde_json::Value,
}

/// Knobs a provider may or may not honour. Unsupported options are reported
/// through [`Capabilities`], never silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderOptions {
    /// Reasoning effort (`low`, `medium`, `high`, `xhigh`, `max`).
    pub effort: Option<String>,
    /// Ask the provider to cache the system prefix.
    pub cache_prefix: bool,
    /// Sampling temperature, where the provider allows it.
    pub temperature: Option<f32>,
    /// Free-form provider-specific extras (e.g. a gateway's routing goal).
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Model alias; the router resolves it. Adapters receive the resolved name in `model`.
    pub alias: ModelAlias,
    /// Concrete model name once resolved; empty before routing.
    pub model: String,
    /// System prefix, kept byte-stable for caching.
    pub system: String,
    /// Transcript.
    pub messages: Vec<Message>,
    /// Tools the model may call.
    pub tools: Vec<ToolDef>,
    /// JSON schema the output must validate against, for `LLM_JSON` / `LLM_BOOL`.
    pub output_schema: Option<serde_json::Value>,
    /// Output cap.
    pub max_tokens: u32,
    /// Provider knobs.
    pub options: ProviderOptions,
}

impl CompletionRequest {
    /// Content-addressed key for the memo and replay fixtures: everything that
    /// affects the answer, nothing that does not (no ids, no timestamps).
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.alias.0.as_bytes());
        h.update(b"\0");
        h.update(self.model.as_bytes());
        h.update(b"\0");
        h.update(self.system.as_bytes());
        h.update(b"\0");
        h.update(serde_json::to_vec(&self.messages).unwrap_or_default());
        h.update(serde_json::to_vec(&self.tools).unwrap_or_default());
        h.update(serde_json::to_vec(&self.output_schema).unwrap_or_default());
        h.update(serde_json::to_vec(&self.options).unwrap_or_default());
        h.update(self.max_tokens.to_le_bytes());
        format!("{:x}", h.finalize())
    }
}

/// Why the model stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end.
    EndTurn,
    /// Hit `max_tokens`.
    MaxTokens,
    /// Wants tool results.
    ToolUse,
    /// The provider declined the request.
    Refusal,
    /// Anything else, named. The default until a provider says otherwise.
    #[default]
    Other,
}

/// Token accounting for one call.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Usage {
    /// Input tokens billed at full price.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens served from a prompt cache.
    pub cache_read_tokens: u64,
    /// Input tokens written to a prompt cache.
    pub cache_write_tokens: u64,
    /// Cost as reported by the provider, if it reports one.
    pub cost_usd: Option<f64>,
}

impl Usage {
    /// All tokens, for budget accounting.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// A completed response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Model that actually served (may differ from the request under failover).
    pub model: String,
    /// Content blocks.
    pub content: Vec<ContentBlock>,
    /// Why it stopped.
    pub stop_reason: StopReason,
    /// Tokens and cost.
    pub usage: Usage,
}

impl CompletionResponse {
    /// Concatenated text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Streaming events, provider-neutral.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum StreamEvent {
    /// A piece of output text.
    TextDelta {
        /// The text.
        text: String,
    },
    /// The model started a tool call.
    ToolUseStart {
        /// Id.
        id: String,
        /// Tool name.
        name: String,
    },
    /// Partial JSON of the current tool call's arguments.
    ToolInputDelta {
        /// Partial JSON text.
        partial_json: String,
    },
    /// The final assembled response.
    Done(CompletionResponse),
}

/// What a backend can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    /// Streams tokens.
    pub streaming: bool,
    /// Supports tool definitions and tool-use blocks.
    pub tools: bool,
    /// Constrains output to a JSON schema natively.
    pub json_schema: bool,
    /// Honours a prompt-cache request and reports cache reads.
    pub prompt_cache: bool,
    /// Honours an effort / reasoning setting.
    pub reasoning_control: bool,
    /// Reports cost in USD.
    pub cost_reported: bool,
}

/// Provider failures.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// Bad credentials.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Rate limited; retry after the given seconds if known.
    #[error("rate limited")]
    RateLimited {
        /// Suggested wait.
        retry_after_secs: Option<u64>,
    },
    /// The request was rejected as invalid.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Transport failure.
    #[error("network error: {0}")]
    Network(String),
    /// The provider returned something we could not parse.
    #[error("malformed response: {0}")]
    Malformed(String),
    /// No fixture recorded for this request.
    #[error("no replay fixture for fingerprint {0}")]
    NoFixture(String),
    /// Every candidate for an alias failed.
    #[error("all candidates for alias {alias} failed: {last}")]
    Exhausted {
        /// The alias.
        alias: String,
        /// Last error text.
        last: String,
    },
    /// Alias not configured.
    #[error("unknown model alias: {0}")]
    UnknownAlias(String),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(text: &str) -> CompletionRequest {
        CompletionRequest {
            alias: ModelAlias::worker(),
            model: "m".into(),
            system: "sys".into(),
            messages: vec![Message::user(text)],
            tools: vec![],
            output_schema: None,
            max_tokens: 100,
            options: ProviderOptions::default(),
        }
    }

    #[test]
    fn fingerprint_depends_only_on_content() {
        assert_eq!(req("a").fingerprint(), req("a").fingerprint());
        assert_ne!(req("a").fingerprint(), req("b").fingerprint());
    }
}
