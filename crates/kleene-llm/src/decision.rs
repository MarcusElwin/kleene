//! Typed decisions: the request and answer shapes of a *decision model*
//! such as TypeSafe's Jev, and the [`DecisionProvider`] trait that backends
//! implement.
//!
//! A decision model is not a text model. It takes a JSON `state` (the thing
//! to decide about) and named questions of three kinds, and returns typed
//! answers with probabilities instead of prose:
//!
//! | question | answer |
//! |---|---|
//! | [`Question::Noul`], a yes/no question | probability in `[0, 1]` that the answer is yes |
//! | [`Question::Choice`], pick one of named labels | the winning label, its confidence, a probability per label |
//! | [`Question::Score`], rate on an ordered rubric | the expected level (fractional), its confidence, a probability per level |
//!
//! Every question in one request is answered in one round trip. The harness
//! uses this for cheap typed judgements inside relational operators (a
//! filter, an `ORDER BY`, a cascade proxy) where a text call would be slow
//! and expensive, and where a probability is more useful than a sentence.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::types::{ProviderError, Usage};

/// The name every decision request is fingerprinted and memoised under;
/// also the model alias ([`ModelAlias::jev`](kleene_core::ModelAlias::jev))
/// the catalog uses for decision functions.
pub const DECISION_ALIAS: &str = "jev";

/// One question about the state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// A yes/no question or statement.
    Noul {
        /// What to decide, as text or structured JSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<serde_json::Value>,
        /// Optional descriptions of the yes (`true`) and no (`false`) outcomes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<serde_json::Value>,
    },
    /// Pick one label.
    Choice {
        /// What to decide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<serde_json::Value>,
        /// Labels and an optional description of when each applies; `null`
        /// leaves a label to be read by its name alone. Ordered so the wire
        /// form and the fingerprint are stable.
        criteria: BTreeMap<String, Option<serde_json::Value>>,
    },
    /// Rate on an ordered rubric.
    Score {
        /// What to decide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<serde_json::Value>,
        /// One description per level, from level 0 upwards; at least one.
        criteria: Vec<serde_json::Value>,
    },
}

impl Question {
    /// A yes/no question in plain words.
    pub fn noul(instructions: impl Into<String>) -> Self {
        Self::Noul {
            instructions: Some(serde_json::Value::String(instructions.into())),
            criteria: None,
        }
    }

    /// A choice between labels read by name alone.
    pub fn choice<I, S>(instructions: impl Into<String>, labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Choice {
            instructions: Some(serde_json::Value::String(instructions.into())),
            criteria: labels.into_iter().map(|l| (l.into(), None)).collect(),
        }
    }

    /// A score over levels described in words, level 0 first.
    pub fn score<I, S>(instructions: impl Into<String>, levels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Score {
            instructions: Some(serde_json::Value::String(instructions.into())),
            criteria: levels
                .into_iter()
                .map(|l| serde_json::Value::String(l.into()))
                .collect(),
        }
    }

    /// The wire tag: `noul`, `choice` or `score`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Noul { .. } => "noul",
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
        }
    }
}

/// One decision request: a state and the questions to answer about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// The content to decide about: text, an object or an array.
    pub state: serde_json::Value,
    /// Model name or alias; empty means the provider's default.
    #[serde(default)]
    pub model: String,
    /// Questions by name; the answers come back under the same names.
    pub questions: BTreeMap<String, Question>,
}

impl DecisionRequest {
    /// One question about a state, under the name `q`.
    pub fn single(state: serde_json::Value, question: Question) -> Self {
        Self {
            state,
            model: String::new(),
            questions: BTreeMap::from([("q".to_string(), question)]),
        }
    }

    /// Content-addressed key for the memo and replay fixtures: the state,
    /// the model and the questions, nothing else.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(DECISION_ALIAS.as_bytes());
        h.update(b"\0");
        h.update(self.model.as_bytes());
        h.update(b"\0");
        h.update(serde_json::to_vec(&self.state).unwrap_or_default());
        h.update(b"\0");
        h.update(serde_json::to_vec(&self.questions).unwrap_or_default());
        format!("{:x}", h.finalize())
    }
}

/// One typed answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    /// The answer to a [`Question::Noul`].
    Noul {
        /// Probability of yes, in `[0, 1]`.
        noul: f64,
    },
    /// The answer to a [`Question::Choice`].
    Choice {
        /// The most probable label.
        choice: String,
        /// Confidence in that label, in `[0, 1]`.
        confidence: f64,
        /// Probability of every label; sums to about 1.
        probabilities: BTreeMap<String, f64>,
    },
    /// The answer to a [`Question::Score`].
    Score {
        /// Expected level, from 0 to `levels - 1`, fractional between levels.
        score: f64,
        /// Confidence in the score, in `[0, 1]`.
        confidence: f64,
        /// The rubric as sent, keyed by level.
        #[serde(default)]
        legend: BTreeMap<String, serde_json::Value>,
        /// Probability of every level, keyed by level.
        probabilities: BTreeMap<String, f64>,
    },
}

impl Answer {
    /// The wire tag: `noul`, `choice` or `score`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Noul { .. } => "noul",
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
        }
    }

    /// The answer as one number: the yes probability, the choice's
    /// confidence, or the expected score.
    pub fn as_f64(&self) -> f64 {
        match self {
            Self::Noul { noul } => *noul,
            Self::Choice { confidence, .. } => *confidence,
            Self::Score { score, .. } => *score,
        }
    }
}

/// The answers to one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// The model that answered.
    pub model: String,
    /// Answers by question name.
    pub answers: BTreeMap<String, Answer>,
    /// Tokens and cost.
    #[serde(default)]
    pub usage: Usage,
}

impl DecisionResponse {
    /// The answer to the single question of [`DecisionRequest::single`].
    pub fn single(&self) -> Result<&Answer, ProviderError> {
        self.answers
            .get("q")
            .or_else(|| self.answers.values().next())
            .ok_or_else(|| ProviderError::Malformed("decision response has no answers".into()))
    }
}

/// A decision-model backend.
#[async_trait::async_trait]
pub trait DecisionProvider: Send + Sync {
    /// Stable identifier (`typesafe`, `replay`, `scripted`).
    fn name(&self) -> &str;
    /// Answer every question in the request.
    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn questions_serialise_in_the_wire_shape() {
        let q = Question::choice("tone?", ["calm", "angry"]);
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type": "choice", "instructions": "tone?", "criteria": {"angry": null, "calm": null}})
        );
        let q = Question::noul("billing?");
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type": "noul", "instructions": "billing?"})
        );
        let q = Question::score("how bad?", ["fine", "bad"]);
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type": "score", "instructions": "how bad?", "criteria": ["fine", "bad"]})
        );
    }

    #[test]
    fn answers_deserialise_by_tag() {
        let a: Answer = serde_json::from_value(json!({"type": "noul", "noul": 0.9})).unwrap();
        assert_eq!(a.as_f64(), 0.9);
        let a: Answer = serde_json::from_value(json!({
            "type": "score", "score": 1.5, "confidence": 0.6,
            "legend": {"0": "fine", "1": "bad"}, "probabilities": {"0": 0.5, "1": 0.5}
        }))
        .unwrap();
        assert_eq!(a.kind(), "score");
        assert_eq!(a.as_f64(), 1.5);
    }

    #[test]
    fn fingerprint_depends_only_on_content() {
        let a = DecisionRequest::single(json!("x"), Question::noul("ok?"));
        let b = DecisionRequest::single(json!("x"), Question::noul("ok?"));
        let c = DecisionRequest::single(json!("y"), Question::noul("ok?"));
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }
}
