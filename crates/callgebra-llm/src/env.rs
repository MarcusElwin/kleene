//! Building the provider stack from environment variables.

use crate::adapters::{AnthropicProvider, OpenAiCompatProvider};
use crate::router::{RoutedProvider, RouterConfig};
use crate::types::ProviderError;
use crate::Provider;
use std::collections::HashMap;
use std::sync::Arc;

/// Path of the router TOML, read when set.
pub const ROUTER_TOML_ENV: &str = "CALLGEBRA_ROUTER_TOML";
/// Model used for every alias when only an OpenAI-compatible endpoint is
/// configured and no router TOML is given.
pub const OPENAI_MODEL_ENV: &str = "OPENAI_MODEL";
/// Fallback for [`OPENAI_MODEL_ENV`].
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4-mini";

fn set(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Build a [`RoutedProvider`] from the environment.
///
/// Registers the Anthropic adapter when `ANTHROPIC_API_KEY` or
/// `ANTHROPIC_AUTH_TOKEN` is set, and the OpenAI-compatible adapter when
/// `OPENAI_API_KEY` or `OPENAI_BASE_URL` is set. Routing comes from the file
/// named by `CALLGEBRA_ROUTER_TOML` if present, otherwise
/// [`RouterConfig::default_for_anthropic`], or, with only an OpenAI-compatible
/// endpoint, [`RouterConfig::default_for_openai_compat`] on `OPENAI_MODEL`
/// (default `gpt-5.4-mini`). Errors with [`ProviderError::Other`] when no
/// backend is configured.
pub fn provider_from_env() -> Result<Arc<dyn Provider>, ProviderError> {
    let has_anthropic = set("ANTHROPIC_API_KEY") || set("ANTHROPIC_AUTH_TOKEN");
    let has_openai = set("OPENAI_API_KEY") || set("OPENAI_BASE_URL");
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    if has_anthropic {
        let p = AnthropicProvider::from_env()?;
        providers.insert(p.name().to_string(), Arc::new(p));
    }
    if has_openai {
        let p = OpenAiCompatProvider::from_env()?;
        providers.insert(p.name().to_string(), Arc::new(p));
    }
    if providers.is_empty() {
        return Err(ProviderError::Other(
            "no model provider configured: set ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) \
             for Anthropic, or OPENAI_API_KEY / OPENAI_BASE_URL for an OpenAI-compatible \
             endpoint; optionally CALLGEBRA_ROUTER_TOML for alias routing"
                .into(),
        ));
    }
    let config = match std::env::var(ROUTER_TOML_ENV)
        .ok()
        .filter(|p| !p.is_empty())
    {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                ProviderError::Other(format!("cannot read {ROUTER_TOML_ENV}={path}: {e}"))
            })?;
            RouterConfig::from_toml(&text)?
        }
        None if has_anthropic => RouterConfig::default_for_anthropic(),
        None => {
            let model = std::env::var(OPENAI_MODEL_ENV)
                .ok()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
            RouterConfig::default_for_openai_compat(&model)
        }
    };
    Ok(Arc::new(RoutedProvider::new(config, providers)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_is_a_clear_error() {
        if [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
        ]
        .iter()
        .any(|v| set(v))
        {
            return;
        }
        match provider_from_env() {
            Err(ProviderError::Other(m)) => assert!(m.contains("ANTHROPIC_API_KEY"), "{m}"),
            Err(other) => panic!("unexpected error kind: {other}"),
            Ok(_) => panic!("built a provider with nothing configured"),
        }
    }
}
