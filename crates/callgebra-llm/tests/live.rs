//! Smoke tests against real endpoints. Ignored by default: they need network
//! access and credentials. Run with
//!
//! ```text
//! ANTHROPIC_API_KEY=... cargo test -p callgebra-llm --test live -- --ignored
//! ```
//!
//! `CALLGEBRA_LIVE_ANTHROPIC_MODEL` and `CALLGEBRA_LIVE_OPENAI_MODEL` pick the
//! models (defaults `claude-sonnet-5` and `OPENAI_MODEL` / `gpt-5.4-mini`).

use callgebra_llm::{
    AnthropicProvider, CompletionRequest, Message, OpenAiCompatProvider, Provider, ProviderOptions,
    StopReason, StreamEvent,
};
use futures::StreamExt;
use serde_json::json;

fn request(model: &str) -> CompletionRequest {
    CompletionRequest {
        alias: "worker".into(),
        model: model.into(),
        system: "Answer with a single word.".into(),
        messages: vec![Message::user("What colour is the sky on a clear day?")],
        tools: vec![],
        output_schema: None,
        max_tokens: 64,
        options: ProviderOptions {
            effort: Some("low".into()),
            cache_prefix: true,
            ..Default::default()
        },
    }
}

async fn round_trip(p: &dyn Provider, model: &str) {
    let resp = p.complete(request(model)).await.expect("complete");
    assert_eq!(resp.stop_reason, StopReason::EndTurn, "{resp:?}");
    assert!(resp.text().to_lowercase().contains("blue"), "{resp:?}");
    assert!(resp.usage.output_tokens > 0);

    let mut json_req = request(model);
    json_req.output_schema = Some(json!({
        "type": "object",
        "properties": {"colour": {"type": "string"}},
        "required": ["colour"],
        "additionalProperties": false
    }));
    let resp = p.complete(json_req).await.expect("structured complete");
    let v: serde_json::Value = serde_json::from_str(&resp.text()).expect("valid JSON");
    assert!(v["colour"].is_string(), "{v}");

    let events: Vec<_> = p
        .stream(request(model))
        .await
        .expect("stream")
        .map(|e| e.expect("stream event"))
        .collect()
        .await;
    let Some(StreamEvent::Done(done)) = events.last() else {
        panic!("stream must end with Done: {events:?}");
    };
    assert!(done.text().to_lowercase().contains("blue"), "{done:?}");
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::TextDelta { .. })));
}

#[tokio::test]
#[ignore = "needs network and ANTHROPIC_API_KEY / ANTHROPIC_AUTH_TOKEN"]
async fn anthropic_round_trip() {
    let p = AnthropicProvider::from_env().expect("Anthropic credentials");
    let model = std::env::var("CALLGEBRA_LIVE_ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-5".into());
    round_trip(&p, &model).await;
}

#[tokio::test]
#[ignore = "needs network and OPENAI_API_KEY / OPENAI_BASE_URL"]
async fn openai_compat_round_trip() {
    let p = OpenAiCompatProvider::from_env().expect("OpenAI-compatible endpoint");
    let model = std::env::var("CALLGEBRA_LIVE_OPENAI_MODEL")
        .or_else(|_| std::env::var("OPENAI_MODEL"))
        .unwrap_or_else(|_| "gpt-5.4-mini".into());
    round_trip(&p, &model).await;
}

#[tokio::test]
#[ignore = "needs network and provider credentials"]
async fn routed_provider_from_env_serves_worker() {
    let p = callgebra_llm::provider_from_env().expect("a configured provider");
    let mut req = request("");
    req.alias = "worker".into();
    let resp = p.complete(req).await.expect("routed complete");
    assert!(!resp.model.is_empty());
    assert!(resp.usage.cost_usd.is_some() || !p.capabilities().cost_reported);
}
