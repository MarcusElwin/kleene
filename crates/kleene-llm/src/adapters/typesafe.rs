//! TypeSafe adapter for Jev, a decision model (`POST {base}/v1/systemone`).
//!
//! The wire format is the one TypeSafe's own SDK speaks: a bearer key, a
//! JSON body of `state`, `model` and `questions`, and a response of
//! `answers` keyed by question name. No SDK; one request per call over
//! `reqwest`, with the same retry policy as the text adapters.

use crate::decision::{DecisionProvider, DecisionRequest, DecisionResponse};
use crate::http::{self, DEFAULT_BACKOFF};
use crate::types::{ProviderError, Usage};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Default API base.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Default model: TypeSafe's rolling alias for the current Jev.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// List price per input token (USD 0.042 per million) at the time of
/// writing; output is free. The adapter reports cost from this so budgets
/// see decisions; override it with [`TypeSafeProvider::with_input_price`].
pub const USD_PER_INPUT_TOKEN: f64 = 0.042e-6;

/// Environment variables the adapter and `kleene setup` read.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Base URL override.
pub const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// Default model override.
pub const DEFAULT_MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";

/// Jev over TypeSafe's HTTP API.
pub struct TypeSafeProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    usd_per_input_token: f64,
    backoff: Vec<Duration>,
}

impl std::fmt::Debug for TypeSafeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeSafeProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

impl TypeSafeProvider {
    /// Talk to `base_url` (default [`DEFAULT_BASE_URL`]) with a bearer key,
    /// answering with `model` (default [`DEFAULT_MODEL`]) when a request
    /// names none.
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            api_key,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            usd_per_input_token: USD_PER_INPUT_TOKEN,
            backoff: DEFAULT_BACKOFF.to_vec(),
        }
    }

    /// Read `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL` and
    /// `TYPESAFE_DEFAULT_MODEL`. Errors with [`ProviderError::Auth`] when the
    /// key is missing.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = non_empty_env(API_KEY_ENV)
            .ok_or_else(|| ProviderError::Auth(format!("{API_KEY_ENV} is not set")))?;
        Ok(Self::new(
            key,
            non_empty_env(BASE_URL_ENV),
            non_empty_env(DEFAULT_MODEL_ENV),
        ))
    }

    /// Override the retry backoff schedule (one retry per entry).
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }

    /// Override the price used to report cost.
    pub fn with_input_price(mut self, usd_per_input_token: f64) -> Self {
        self.usd_per_input_token = usd_per_input_token;
        self
    }

    /// The API base this adapter talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The model used when a request names none.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The JSON body sent for a request. Public so the mapping is testable
    /// and can be logged by the trace.
    pub fn build_body(&self, req: &DecisionRequest) -> Value {
        let model = if req.model.is_empty() {
            self.model.as_str()
        } else {
            req.model.as_str()
        };
        json!({
            "state": req.state,
            "model": model,
            "questions": req.questions,
        })
    }

    /// Decode a response body. Answer kinds this version does not know are
    /// skipped, as the SDK does, so a new primitive cannot break old calls.
    pub fn parse_response(&self, body: &str) -> Result<DecisionResponse, ProviderError> {
        let wire: WireResponse =
            serde_json::from_str(body).map_err(|e| ProviderError::Malformed(e.to_string()))?;
        let mut answers = std::collections::BTreeMap::new();
        for (name, raw) in wire.answers {
            match serde_json::from_value(raw.clone()) {
                Ok(a) => {
                    answers.insert(name, a);
                }
                Err(e) => {
                    let kind = raw.get("type").and_then(|t| t.as_str()).unwrap_or("?");
                    if matches!(kind, "noul" | "choice" | "score") {
                        return Err(ProviderError::Malformed(format!(
                            "answer {name} ({kind}): {e}"
                        )));
                    }
                    tracing::warn!(answer = %name, kind, "ignoring an answer of an unknown kind");
                }
            }
        }
        let input_tokens = wire.usage.input_tokens.unwrap_or(0);
        Ok(DecisionResponse {
            model: wire.model.unwrap_or_default(),
            answers,
            usage: Usage {
                input_tokens,
                output_tokens: wire.usage.output_tokens.unwrap_or(0),
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(input_tokens as f64 * self.usd_per_input_token),
            },
        })
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1/systemone", self.base_url))
            .bearer_auth(&self.api_key)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(body)
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    answers: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

#[async_trait::async_trait]
impl DecisionProvider for TypeSafeProvider {
    fn name(&self) -> &str {
        "typesafe"
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, ProviderError> {
        if req.questions.is_empty() {
            return Err(ProviderError::BadRequest(
                "a decision request needs at least one question".into(),
            ));
        }
        let body = self.build_body(&req);
        let resp = http::send(&self.backoff, || self.request(&body)).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        self.parse_response(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{Answer, Question};

    fn provider() -> TypeSafeProvider {
        TypeSafeProvider::new("k".into(), None, None)
    }

    #[test]
    fn body_has_state_model_and_questions() {
        let mut req = DecisionRequest::single(
            json!("I was charged twice."),
            Question::noul("Is this about billing?"),
        );
        req.questions.insert(
            "tone".into(),
            Question::choice("What is the tone?", ["calm", "angry"]),
        );
        let body = provider().build_body(&req);
        assert_eq!(body["model"], json!("jev-latest"));
        assert_eq!(body["state"], json!("I was charged twice."));
        assert_eq!(body["questions"]["q"]["type"], json!("noul"));
        assert_eq!(
            body["questions"]["tone"]["criteria"],
            json!({"angry": null, "calm": null})
        );
        req.model = "jev-2".into();
        assert_eq!(provider().build_body(&req)["model"], json!("jev-2"));
    }

    #[test]
    fn parses_every_answer_kind_and_prices_input_tokens() {
        let resp = provider()
            .parse_response(include_str!("../../tests/fixtures/typesafe_answers.json"))
            .unwrap();
        assert_eq!(resp.model, "jev-latest");
        assert_eq!(resp.answers.len(), 3);
        assert!(matches!(
            resp.answers["billing"],
            Answer::Noul { noul } if (noul - 0.98).abs() < 1e-9
        ));
        let Answer::Choice {
            choice,
            confidence,
            probabilities,
        } = &resp.answers["tone"]
        else {
            panic!("tone is a choice")
        };
        assert_eq!(choice, "angry");
        assert!((confidence - 0.9).abs() < 1e-9);
        assert_eq!(probabilities.len(), 2);
        let Answer::Score {
            score,
            legend,
            probabilities,
            ..
        } = &resp.answers["severity"]
        else {
            panic!("severity is a score")
        };
        assert!((score - 1.3).abs() < 1e-9);
        assert_eq!(legend["0"], json!("minor"));
        assert_eq!(probabilities.len(), 3);
        assert_eq!(resp.usage.input_tokens, 120);
        assert_eq!(resp.usage.output_tokens, 0);
        assert!((resp.usage.cost_usd.unwrap() - 120.0 * USD_PER_INPUT_TOKEN).abs() < 1e-15);
    }

    #[test]
    fn unknown_answer_kinds_are_skipped_and_bad_known_ones_rejected() {
        let ok = provider()
            .parse_response(
                r#"{"model":"m","answers":{"a":{"type":"noul","noul":0.5},"b":{"type":"rank","order":[]}},"usage":{"input_tokens":1,"output_tokens":0}}"#,
            )
            .unwrap();
        assert_eq!(ok.answers.len(), 1);
        assert!(matches!(
            provider().parse_response(r#"{"answers":{"a":{"type":"noul"}}}"#),
            Err(ProviderError::Malformed(m)) if m.contains("answer a")
        ));
        assert!(matches!(
            provider().parse_response("{not json"),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed_and_env_needs_a_key() {
        let p = TypeSafeProvider::new("k".into(), Some("https://proxy.example/".into()), None);
        assert_eq!(p.base_url(), "https://proxy.example");
        assert_eq!(p.model(), DEFAULT_MODEL);
        if std::env::var_os(API_KEY_ENV).is_none() {
            assert!(matches!(
                TypeSafeProvider::from_env(),
                Err(ProviderError::Auth(_))
            ));
        }
    }
}
