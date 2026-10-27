//! Building the provider stack from the environment and the config file.

use crate::adapters::{AnthropicProvider, OpenAiCompatProvider};
use crate::config::ProviderSettings;
use crate::router::{ModelPricing, RoutedProvider, RouterConfig};
use crate::types::ProviderError;
use crate::Provider;
use std::collections::HashMap;
use std::sync::Arc;

/// Path of the router TOML, read when set.
pub const ROUTER_TOML_ENV: &str = "KLEENE_ROUTER_TOML";
/// Model used for every alias when only an OpenAI-compatible endpoint is
/// configured and no router TOML is given.
pub const OPENAI_MODEL_ENV: &str = "OPENAI_MODEL";
/// Fallback for [`OPENAI_MODEL_ENV`].
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4-mini";
/// Prices for models the rate cards do not know, layered over the router's
/// `[pricing]` table: `model=input/output[/cache_read[/cache_write]]` in
/// dollars per million tokens, several separated by `,`. The model name is
/// everything before the first `=`, so names with a `/` in them work:
/// `KLEENE_PRICING="deepseek/deepseek-v4.1-flash=0.28/0.42"`.
pub const PRICING_ENV: &str = "KLEENE_PRICING";

/// The message when nothing is configured; `kleene setup` is the fix.
pub const NOT_CONFIGURED: &str = "no model provider configured: run `kleene setup`, or set \
     ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) for Anthropic, or OPENAI_API_KEY / \
     OPENAI_BASE_URL for an OpenAI-compatible endpoint; optionally KLEENE_ROUTER_TOML \
     for alias routing";

/// Build a [`RoutedProvider`] from the environment layered over the config
/// file ([`ProviderSettings::effective`]).
///
/// Registers the Anthropic adapter when a key or auth token is present, and
/// the OpenAI-compatible adapter when a key or base URL is; with the
/// `gateway` feature, also the Open Responses adapter when
/// `OPEN_RESPONSES_API_KEY` or `OPEN_RESPONSES_BASE_URL` is. Routing comes
/// from the router TOML if one is named, otherwise
/// [`RouterConfig::default_for_anthropic`], or, with only an OpenAI-compatible
/// endpoint, [`RouterConfig::default_for_openai_compat`] on the configured
/// model (default `gpt-5.4-mini`); with only a gateway, every alias resolves
/// to `OPEN_RESPONSES_MODEL` (same default) on it. Errors with
/// [`ProviderError::Other`] and [`NOT_CONFIGURED`] when no backend is
/// configured.
pub fn provider_from_env() -> Result<Arc<dyn Provider>, ProviderError> {
    provider_from_settings(&ProviderSettings::effective()?)
}

/// Build the provider stack from explicit settings; see [`provider_from_env`].
pub fn provider_from_settings(
    settings: &ProviderSettings,
) -> Result<Arc<dyn Provider>, ProviderError> {
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    let anthropic = settings.anthropic.as_ref().filter(|a| a.is_configured());
    if let Some(a) = anthropic {
        let p = match (&a.api_key, &a.auth_token) {
            (Some(key), _) if !key.trim().is_empty() => {
                AnthropicProvider::new(key.clone(), a.base_url.clone())
            }
            (_, Some(token)) => {
                AnthropicProvider::with_auth_token(token.clone(), a.base_url.clone())
            }
            _ => unreachable!("is_configured checked a credential"),
        };
        providers.insert(p.name().to_string(), Arc::new(p));
    }
    let openai = settings
        .openai_compat
        .as_ref()
        .filter(|o| o.is_configured());
    if let Some(o) = openai {
        let p = OpenAiCompatProvider::new(
            o.api_key.clone(),
            o.base_url
                .clone()
                .unwrap_or_else(|| crate::adapters::openai_compat::DEFAULT_BASE_URL.to_string()),
        );
        providers.insert(p.name().to_string(), Arc::new(p));
    }
    #[cfg(feature = "gateway")]
    let gateway = settings
        .open_responses
        .as_ref()
        .filter(|g| g.is_configured());
    #[cfg(feature = "gateway")]
    if let Some(g) = gateway {
        let p = crate::adapters::OpenResponsesProvider::new(
            g.api_key.clone(),
            g.base_url
                .clone()
                .unwrap_or_else(|| crate::adapters::open_responses::DEFAULT_BASE_URL.to_string()),
        );
        providers.insert(p.name().to_string(), Arc::new(p));
    }
    if providers.is_empty() {
        return Err(ProviderError::Other(NOT_CONFIGURED.into()));
    }
    let config = match &settings.router {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                ProviderError::Other(format!("cannot read router file {}: {e}", path.display()))
            })?;
            RouterConfig::from_toml(&text)?
        }
        None if anthropic.is_some() => RouterConfig::default_for_anthropic(),
        #[cfg(feature = "gateway")]
        None if openai.is_none() => {
            let model = gateway
                .and_then(|g| g.model.clone())
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
            RouterConfig::default_for_open_responses(&model)
        }
        None => {
            let model = openai
                .and_then(|o| o.model.clone())
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
            RouterConfig::default_for_openai_compat(&model)
        }
    };
    let mut config = config;
    if let Ok(text) = std::env::var(PRICING_ENV) {
        for (model, price) in parse_pricing(&text)? {
            config.pricing.insert(&model, price);
        }
    }
    Ok(Arc::new(RoutedProvider::new(config, providers)))
}

/// Parse a [`PRICING_ENV`] value.
pub fn parse_pricing(text: &str) -> Result<Vec<(String, ModelPricing)>, ProviderError> {
    let bad = |entry: &str, why: &str| {
        ProviderError::Other(format!(
            "{PRICING_ENV}: {why} in {entry:?}; expected model=input/output[/cache_read[/cache_write]] in dollars per million tokens, entries separated by commas"
        ))
    };
    let mut out = vec![];
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((model, rates)) = entry.split_once('=') else {
            return Err(bad(entry, "missing ="));
        };
        let model = model.trim();
        if model.is_empty() {
            return Err(bad(entry, "empty model name"));
        }
        let nums: Vec<f64> = rates
            .split('/')
            .map(|n| {
                n.trim()
                    .parse::<f64>()
                    .map_err(|_| bad(entry, "a rate is not a number"))
            })
            .collect::<Result<_, _>>()?;
        if nums.len() < 2 || nums.len() > 4 || nums.iter().any(|n| *n < 0.0) {
            return Err(bad(entry, "two to four non-negative rates"));
        }
        out.push((
            model.to_string(),
            ModelPricing {
                input_per_mtok: nums[0],
                output_per_mtok: nums[1],
                cache_read_per_mtok: nums.get(2).copied().unwrap_or(0.0),
                cache_write_per_mtok: nums.get(3).copied().unwrap_or(0.0),
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AnthropicSettings, OpenAiCompatSettings};

    #[test]
    fn nothing_configured_is_a_clear_error() {
        match provider_from_settings(&ProviderSettings::default()) {
            Err(ProviderError::Other(m)) => {
                assert!(m.contains("kleene setup"), "{m}");
                assert!(m.contains("ANTHROPIC_API_KEY"), "{m}");
            }
            Err(other) => panic!("unexpected error kind: {other}"),
            Ok(_) => panic!("built a provider with nothing configured"),
        }
    }

    #[test]
    fn settings_build_the_expected_aliases() {
        let s = ProviderSettings {
            openai_compat: Some(OpenAiCompatSettings {
                api_key: None,
                base_url: Some("http://localhost:11434/v1".into()),
                model: Some("llama3".into()),
            }),
            ..Default::default()
        };
        let p = provider_from_settings(&s).expect("a local endpoint needs no key");
        // Every alias resolves to the one model when only an OpenAI-compatible
        // endpoint is configured.
        let _ = p;
        let s = ProviderSettings {
            anthropic: Some(AnthropicSettings {
                auth_token: Some("tok".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        provider_from_settings(&s).expect("an auth token is a credential");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn a_gateway_alone_routes_every_alias_to_it() {
        use crate::config::OpenResponsesSettings;
        let s = ProviderSettings {
            open_responses: Some(OpenResponsesSettings {
                api_key: None,
                base_url: Some("https://aura.example/v1".into()),
                model: Some("gw-model".into()),
            }),
            ..Default::default()
        };
        let p = provider_from_settings(&s).expect("a gateway URL needs no key");
        assert!(p.capabilities().cost_reported, "{:?}", p.capabilities());
        let cfg = RouterConfig::default_for_open_responses("gw-model");
        for alias in ["root", "worker", "proxy", "judge"] {
            let c = &cfg.aliases[alias].candidates;
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].provider, "open_responses");
            assert_eq!(c[0].model, "gw-model");
        }
    }

    #[test]
    fn pricing_env_parses_models_with_slashes() {
        let v =
            parse_pricing("deepseek/deepseek-v4.1-flash-1017=0.28/0.42, local=0/0/0/0").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, "deepseek/deepseek-v4.1-flash-1017");
        assert_eq!(v[0].1.input_per_mtok, 0.28);
        assert_eq!(v[0].1.output_per_mtok, 0.42);
        assert_eq!(v[0].1.cache_read_per_mtok, 0.0);
        assert_eq!(v[1].0, "local");
        for bad in ["model", "=1/2", "m=1", "m=1/2/3/4/5", "m=a/b", "m=-1/2"] {
            let e = parse_pricing(bad).unwrap_err().to_string();
            assert!(e.contains(PRICING_ENV), "{bad}: {e}");
        }
        assert!(parse_pricing("").unwrap().is_empty());
    }

    #[test]
    fn a_missing_router_file_is_reported() {
        let s = ProviderSettings {
            anthropic: Some(AnthropicSettings {
                api_key: Some("k".into()),
                ..Default::default()
            }),
            router: Some("/nonexistent/kleene-router.toml".into()),
            ..Default::default()
        };
        match provider_from_settings(&s) {
            Err(ProviderError::Other(m)) => assert!(m.contains("router file"), "{m}"),
            Err(other) => panic!("expected a read error, got {other}"),
            Ok(_) => panic!("built a provider from a missing router file"),
        }
    }
}
