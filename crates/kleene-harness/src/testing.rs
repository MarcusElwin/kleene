//! Deterministic providers for tests and demos without a model: a text
//! provider and a decision provider.

use kleene_llm::{
    Answer, Capabilities, CompletionRequest, CompletionResponse, ContentBlock, DecisionProvider,
    DecisionRequest, DecisionResponse, Provider, ProviderError, Question, StopReason, Usage,
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

/// A deterministic decision provider (the `jev` alias) for tests: answers
/// from `(needle, response)` rules matched against the state and the
/// question text; the first matching rule wins, else `fallback`. A response
/// that parses as a number is the yes probability of a noul, the probability
/// of the first label of a choice, or the expected level of a score; any
/// other text names the winning label of a choice (at 0.9) or the level of
/// a score. Counts calls.
pub struct ScriptedDecisions {
    rules: Vec<(String, String)>,
    fallback: String,
    calls: AtomicU64,
}

impl ScriptedDecisions {
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

    fn answer(response: &str, q: &Question) -> Answer {
        let number = response.trim().parse::<f64>().ok();
        match q {
            Question::Noul { .. } => Answer::Noul {
                noul: number.unwrap_or(0.5),
            },
            Question::Choice { criteria, .. } => {
                let labels: Vec<&String> = criteria.keys().collect();
                let (winner, p) = match number {
                    Some(p) => (labels.first().map(|l| l.as_str()).unwrap_or(""), p),
                    None => (response.trim(), 0.9),
                };
                let rest = if labels.len() > 1 {
                    (1.0 - p) / (labels.len() - 1) as f64
                } else {
                    0.0
                };
                Answer::Choice {
                    choice: winner.to_string(),
                    confidence: p,
                    probabilities: labels
                        .iter()
                        .map(|l| (l.to_string(), if l.as_str() == winner { p } else { rest }))
                        .collect(),
                }
            }
            Question::Score { criteria, .. } => {
                let score = number.unwrap_or_else(|| {
                    criteria
                        .iter()
                        .position(|c| c.as_str() == Some(response.trim()))
                        .unwrap_or(0) as f64
                });
                Answer::Score {
                    score,
                    confidence: 0.8,
                    legend: criteria
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (i.to_string(), c.clone()))
                        .collect(),
                    probabilities: criteria
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            (
                                i.to_string(),
                                if i as f64 == score.round() { 1.0 } else { 0.0 },
                            )
                        })
                        .collect(),
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl DecisionProvider for ScriptedDecisions {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let state = match &req.state {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let answers = req
            .questions
            .iter()
            .map(|(name, q)| {
                let instructions = match q {
                    Question::Noul { instructions, .. }
                    | Question::Choice { instructions, .. }
                    | Question::Score { instructions, .. } => instructions
                        .as_ref()
                        .map(|i| {
                            i.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| i.to_string())
                        })
                        .unwrap_or_default(),
                };
                let haystack = format!("{state}\n{instructions}");
                let response = self
                    .rules
                    .iter()
                    .find(|(needle, _)| haystack.contains(needle.as_str()))
                    .map(|(_, r)| r.as_str())
                    .unwrap_or(self.fallback.as_str());
                (name.clone(), Self::answer(response, q))
            })
            .collect();
        Ok(DecisionResponse {
            model: "jev-scripted".into(),
            answers,
            usage: Usage {
                input_tokens: state.len() as u64 / 4,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(0.00001),
            },
        })
    }
}
