//! Model-agnostic provider layer.
//!
//! One request shape ([`CompletionRequest`]) covers everything Kleene needs
//! from a model: a cached system prefix, messages, optional tools, an optional
//! JSON schema for the output, streaming and usage. Adapters map it to each
//! vendor's wire format over `reqwest`, no SDKs: [`AnthropicProvider`] speaks
//! the Messages API and [`OpenAiCompatProvider`] the chat-completions API that
//! everyone else exposes. [`RoutedProvider`] resolves model aliases to ordered
//! `(provider, model)` candidates with failover, circuit breaking and a
//! [`Pricing`] table. [`ReplayProvider`] serves recorded fixtures so tests and
//! re-runs cost nothing. [`provider_from_env`] assembles the stack from the
//! environment.

#![forbid(unsafe_code)]

pub mod adapters;
pub mod config;
pub mod env;
mod http;
pub mod replay;
pub mod router;
pub mod sse;
pub mod types;

pub use adapters::{AnthropicProvider, OpenAiCompatProvider};
pub use config::{AnthropicSettings, OpenAiCompatSettings, ProviderSettings};
pub use env::{provider_from_env, provider_from_settings, NOT_CONFIGURED};
pub use replay::{RecordingProvider, ReplayProvider};
pub use router::{
    AliasConfig, Candidate, CircuitState, ModelPricing, Pricing, RoutedProvider, Router,
    RouterConfig,
};
pub use types::{
    Capabilities, CompletionRequest, CompletionResponse, ContentBlock, Message, ProviderError,
    ProviderOptions, Role, StopReason, StreamEvent, ToolDef, Usage,
};

use futures::stream::BoxStream;

/// A model backend.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Stable identifier (`anthropic`, `openai_compat`, `open_responses`, `replay`).
    fn name(&self) -> &str;
    /// What this backend can do; the planner and harness consult it.
    fn capabilities(&self) -> Capabilities;
    /// One request, one response.
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError>;
    /// Streamed response. The default buffers `complete` into a single final
    /// event, which is enough for the replay provider and for tests.
    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        let resp = self.complete(req).await?;
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            StreamEvent::Done(resp),
        )])))
    }
}
