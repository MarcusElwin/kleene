//! A deterministic provider for tests and demos without a model.

use callgebra_llm::{
    Capabilities, CompletionRequest, CompletionResponse, ContentBlock, Provider, ProviderError,
    StopReason, Usage,
};
use std::sync::atomic::{AtomicU64, Ordering};

/// Answers from a list of `(needle, response)` rules matched against the
/// last user message; the first matching rule wins. Counts calls.
pub struct ScriptedProvider {
    rules: Vec<(String, String)>,
    fallback: String,
    calls: AtomicU64,
}

impl ScriptedProvider {
    /// Build from rules; `fallback` answers when no rule matches.
    pub fn new(rules: Vec<(&str, &str)>, fallback: &str) -> Self {
        Self {
            rules: rules
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            fallback: fallback.to_string(),
            calls: AtomicU64::new(0),
        }
    }

    /// Calls served so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: false,
            tools: false,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: false,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let prompt = req
            .messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = self
            .rules
            .iter()
            .find(|(needle, _)| prompt.contains(needle.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| self.fallback.clone());
        Ok(CompletionResponse {
            model: "scripted".into(),
            content: vec![ContentBlock::Text { text }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: prompt.len() as u64 / 4,
                output_tokens: 8,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(0.001),
            },
        })
    }
}
