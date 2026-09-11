//! Alias routing: `root`, `worker`, `proxy`, `judge` to ordered candidates.
//!
//! The router owns no network code. It resolves an alias to a list of
//! `(provider name, model)` candidates and, in M2, tries them in order with
//! per-endpoint health tracking. Configuration comes from `callgebra.toml`.

use callgebra_core::ModelAlias;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One `(provider, model)` pair an alias may resolve to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Provider name as registered with the harness.
    pub provider: String,
    /// Concrete model identifier for that provider.
    pub model: String,
}

/// Alias table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RouterConfig {
    /// Alias name to ordered candidates; the first healthy one serves.
    pub aliases: BTreeMap<String, Vec<Candidate>>,
}

/// Resolves aliases to candidates.
#[derive(Debug, Clone, Default)]
pub struct Router {
    config: RouterConfig,
}

impl Router {
    /// Build from configuration.
    pub fn new(config: RouterConfig) -> Self {
        Self { config }
    }

    /// Candidates for an alias, in preference order. Empty if unknown.
    pub fn candidates(&self, alias: &ModelAlias) -> &[Candidate] {
        self.config
            .aliases
            .get(&alias.0)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All configured aliases.
    pub fn aliases(&self) -> impl Iterator<Item = &String> {
        self.config.aliases.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_in_order_and_unknown_is_empty() {
        let mut cfg = RouterConfig::default();
        cfg.aliases.insert(
            "worker".into(),
            vec![
                Candidate {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-5".into(),
                },
                Candidate {
                    provider: "openai_compat".into(),
                    model: "local".into(),
                },
            ],
        );
        let r = Router::new(cfg);
        assert_eq!(
            r.candidates(&ModelAlias::worker())[0].model,
            "claude-sonnet-5"
        );
        assert!(r.candidates(&ModelAlias::judge()).is_empty());
    }
}
