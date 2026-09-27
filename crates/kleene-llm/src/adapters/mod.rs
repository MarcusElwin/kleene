//! Wire-format adapters. Each maps [`CompletionRequest`](crate::CompletionRequest)
//! to one vendor's HTTP API over `reqwest` and back; no vendor SDKs.

pub mod anthropic;
#[cfg(feature = "gateway")]
pub mod open_responses;
pub mod openai_compat;

pub use anthropic::AnthropicProvider;
#[cfg(feature = "gateway")]
pub use open_responses::OpenResponsesProvider;
pub use openai_compat::OpenAiCompatProvider;

/// An `f32` option as a JSON number with its shortest decimal form, so a
/// temperature of `0.2` goes on the wire as `0.2` rather than the widened
/// `0.20000000298023224`.
pub(crate) fn f32_json(v: f32) -> serde_json::Value {
    v.to_string()
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::f32_json;
    use serde_json::json;

    #[test]
    fn f32_keeps_its_shortest_form() {
        assert_eq!(f32_json(0.2), json!(0.2));
        assert_eq!(f32_json(1.0), json!(1.0));
        assert_eq!(f32_json(0.0), json!(0.0));
    }
}
