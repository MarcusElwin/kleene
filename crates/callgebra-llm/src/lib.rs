//! Model-agnostic provider layer.
//!
//! One request shape ([`CompletionRequest`]) covers everything Callgebra needs
//! from a model: a cached system prefix, messages, optional tools, an optional
//! JSON schema for the output, streaming and usage. Adapters map it to each
//! vendor's wire format (M2). The [`Router`] resolves model aliases to ordered
//! `(provider, model)` candidates. [`ReplayProvider`] serves recorded fixtures
//! so tests and re-runs cost nothing.

#![forbid(unsafe_code)]

mod http;
pub mod replay;
pub mod router;
pub mod sse;
pub mod types;

pub use replay::{RecordingProvider, ReplayProvider};
pub use router::{Candidate, Router, RouterConfig};
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
