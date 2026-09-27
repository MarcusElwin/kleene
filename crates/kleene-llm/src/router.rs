//! Alias routing: `root`, `worker`, `proxy`, `judge` to ordered candidates,
//! with failover, per-endpoint circuit breakers and a pricing table.
//!
//! [`Router`] is the pure part: it resolves an alias to `(provider, model)`
//! candidates and per-alias default options. [`RoutedProvider`] wraps it
//! around a set of real backends and implements [`Provider`] itself, so the
//! executor only ever sees one provider. Configuration comes from
//! `kleene.toml`; see [`RouterConfig::from_toml`].

use crate::types::{
    Capabilities, CompletionRequest, CompletionResponse, ProviderError, ProviderOptions,
    StreamEvent, Usage,
};
use crate::Provider;
use futures::stream::{BoxStream, StreamExt};
use kleene_core::ModelAlias;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One `(provider, model)` pair an alias may resolve to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Provider name as registered with the harness.
    pub provider: String,
    /// Concrete model identifier for that provider.
    pub model: String,
}

impl Candidate {
    /// Shorthand constructor.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    fn key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// Price per million tokens for one model. Missing fields are zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelPricing {
    /// Uncached input tokens.
    pub input_per_mtok: f64,
    /// Output tokens.
    pub output_per_mtok: f64,
    /// Input tokens read from the prompt cache.
    pub cache_read_per_mtok: f64,
    /// Input tokens written to the prompt cache.
    pub cache_write_per_mtok: f64,
}

impl ModelPricing {
    /// Cost in USD of one call's usage at these rates.
    pub fn cost(&self, usage: &Usage) -> f64 {
        (usage.input_tokens as f64 * self.input_per_mtok
            + usage.output_tokens as f64 * self.output_per_mtok
            + usage.cache_read_tokens as f64 * self.cache_read_per_mtok
            + usage.cache_write_tokens as f64 * self.cache_write_per_mtok)
            / 1_000_000.0
    }
}

/// Pricing table keyed by model name. Actuals for providers that do not
/// report cost, and the planner's estimates, both come from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Pricing {
    /// Model name to rates.
    pub models: HashMap<String, ModelPricing>,
}

impl Pricing {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a model's rates.
    pub fn insert(&mut self, model: impl Into<String>, rates: ModelPricing) {
        self.models.insert(model.into(), rates);
    }

    /// Rates for a model, if known.
    pub fn get(&self, model: &str) -> Option<&ModelPricing> {
        self.models.get(model)
    }

    /// No models priced.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Cost in USD of `usage` on `model`; `None` when the model is not priced.
    pub fn cost(&self, model: &str, usage: &Usage) -> Option<f64> {
        self.models.get(model).map(|p| p.cost(usage))
    }
}

/// Where an alias goes and what options it carries by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AliasConfig {
    /// Ordered candidates; the first healthy one serves.
    pub candidates: Vec<Candidate>,
    /// Defaults applied where a request leaves an option unset.
    #[serde(default)]
    pub options: ProviderOptions,
}

/// Alias table plus pricing: the `[aliases]` and `[pricing]` sections of
/// `kleene.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RouterConfig {
    /// Alias name to its candidates and default options.
    pub aliases: BTreeMap<String, AliasConfig>,
    /// Per-model rates.
    pub pricing: Pricing,
}

impl RouterConfig {
    /// Parse the TOML form.
    ///
    /// ```toml
    /// [aliases.root]
    /// candidates = [{ provider = "anthropic", model = "claude-opus-5" }]
    /// options = { effort = "high" }
    ///
    /// [aliases.worker]
    /// candidates = [
    ///   { provider = "anthropic", model = "claude-sonnet-5" },
    ///   { provider = "openai_compat", model = "gpt-5.4-mini" },
    /// ]
    /// options = { effort = "low" }
    ///
    /// [pricing."claude-opus-5"]
    /// input_per_mtok = 5.0
    /// output_per_mtok = 25.0
    /// cache_read_per_mtok = 0.5
    /// ```
    ///
    /// `options` accepts every [`ProviderOptions`] field (`effort`,
    /// `cache_prefix`, `temperature`, `extra`); omitted ones stay unset.
    /// Pricing fields not given are zero.
    pub fn from_toml(text: &str) -> Result<Self, ProviderError> {
        toml::from_str(text).map_err(|e| ProviderError::Other(format!("router config: {e}")))
    }

    /// Serialise back to TOML.
    pub fn to_toml(&self) -> Result<String, ProviderError> {
        toml::to_string(self).map_err(|e| ProviderError::Other(format!("router config: {e}")))
    }

    /// Add or replace an alias.
    pub fn with_alias(
        mut self,
        alias: impl Into<String>,
        candidates: Vec<Candidate>,
        options: ProviderOptions,
    ) -> Self {
        self.aliases.insert(
            alias.into(),
            AliasConfig {
                candidates,
                options,
            },
        );
        self
    }

    /// Anthropic-only defaults: `root` on Opus 5 at high effort, `worker`
    /// and `judge` on Sonnet 5, `proxy` on Haiku 4.5 at low effort, with the
    /// prompt prefix cached everywhere. Pricing is the first-party rate card
    /// (cache reads at 10% of input, cache writes at 125%).
    pub fn default_for_anthropic() -> Self {
        let opts = |effort: &str| ProviderOptions {
            effort: Some(effort.into()),
            cache_prefix: true,
            ..Default::default()
        };
        let mut pricing = Pricing::new();
        for (model, input, output) in [
            ("claude-opus-5", 5.0, 25.0),
            ("claude-sonnet-5", 2.0, 10.0),
            ("claude-haiku-4-5", 1.0, 5.0),
        ] {
            pricing.insert(
                model,
                ModelPricing {
                    input_per_mtok: input,
                    output_per_mtok: output,
                    cache_read_per_mtok: input * 0.1,
                    cache_write_per_mtok: input * 1.25,
                },
            );
        }
        Self {
            aliases: BTreeMap::new(),
            pricing,
        }
        .with_alias(
            "root",
            vec![Candidate::new("anthropic", "claude-opus-5")],
            opts("high"),
        )
        .with_alias(
            "worker",
            vec![Candidate::new("anthropic", "claude-sonnet-5")],
            opts("medium"),
        )
        .with_alias(
            "proxy",
            vec![Candidate::new("anthropic", "claude-haiku-4-5")],
            opts("low"),
        )
        .with_alias(
            "judge",
            vec![Candidate::new("anthropic", "claude-sonnet-5")],
            opts("medium"),
        )
    }

    /// Single-model defaults for an OpenAI-compatible endpoint: every alias
    /// resolves to `model` on the `openai_compat` provider. No pricing.
    pub fn default_for_openai_compat(model: &str) -> Self {
        let mut cfg = Self::default();
        for alias in ["root", "worker", "proxy", "judge"] {
            cfg = cfg.with_alias(
                alias,
                vec![Candidate::new("openai_compat", model)],
                ProviderOptions::default(),
            );
        }
        cfg
    }

    /// Single-model defaults for an Open Responses gateway: every alias
    /// resolves to `model` on the `open_responses` provider. No pricing; the
    /// gateway reports cost itself.
    #[cfg(feature = "gateway")]
    pub fn default_for_open_responses(model: &str) -> Self {
        let mut cfg = Self::default();
        for alias in ["root", "worker", "proxy", "judge"] {
            cfg = cfg.with_alias(
                alias,
                vec![Candidate::new("open_responses", model)],
                ProviderOptions::default(),
            );
        }
        cfg
    }
}

/// Resolves aliases to candidates and default options.
#[derive(Debug, Clone, Default)]
pub struct Router {
    config: RouterConfig,
}

impl Router {
    /// Build from configuration.
    pub fn new(config: RouterConfig) -> Self {
        Self { config }
    }

    /// The configuration.
    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    /// Candidates for an alias, in preference order. Empty if unknown.
    pub fn candidates(&self, alias: &ModelAlias) -> &[Candidate] {
        self.config
            .aliases
            .get(&alias.0)
            .map(|a| a.candidates.as_slice())
            .unwrap_or(&[])
    }

    /// Default options for an alias, if configured.
    pub fn options(&self, alias: &ModelAlias) -> Option<&ProviderOptions> {
        self.config.aliases.get(&alias.0).map(|a| &a.options)
    }

    /// All configured aliases.
    pub fn aliases(&self) -> impl Iterator<Item = &String> {
        self.config.aliases.keys()
    }
}

/// Fill options the request left unset from the alias defaults.
pub fn apply_defaults(opts: &mut ProviderOptions, defaults: &ProviderOptions) {
    if opts.effort.is_none() {
        opts.effort = defaults.effort.clone();
    }
    if opts.temperature.is_none() {
        opts.temperature = defaults.temperature;
    }
    if !opts.cache_prefix {
        opts.cache_prefix = defaults.cache_prefix;
    }
    for (k, v) in &defaults.extra {
        opts.extra.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Health of one candidate endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Serving normally.
    Closed,
    /// Tripped; requests skip it until the cooldown passes.
    Open,
    /// Cooldown passed; the next request is a trial.
    HalfOpen,
}

#[derive(Debug, Default, Clone)]
struct Circuit {
    failures: u32,
    opened_at: Option<Instant>,
}

/// A [`Provider`] that routes by alias across registered backends.
///
/// On every call it resolves `req.alias`, applies the alias's default
/// options, and tries candidates in order, skipping any whose circuit is
/// open (three consecutive failures open it; after 30 seconds one trial
/// request is let through). Any error moves on to the next candidate; only
/// when every candidate has failed or been skipped does it return
/// [`ProviderError::Exhausted`]. Bad requests fail over too but do not count
/// against an endpoint's health, since they describe the request rather than
/// the backend. When the backend reports no cost, `usage.cost_usd` is filled
/// from the [`Pricing`] table.
pub struct RoutedProvider {
    providers: HashMap<String, Arc<dyn Provider>>,
    router: Router,
    pricing: Pricing,
    health: Mutex<HashMap<String, Circuit>>,
    failure_threshold: u32,
    cooldown: Duration,
}

impl std::fmt::Debug for RoutedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutedProvider")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .field("router", &self.router)
            .finish()
    }
}

impl RoutedProvider {
    /// Default consecutive failures before a circuit opens.
    pub const DEFAULT_FAILURE_THRESHOLD: u32 = 3;
    /// Default time a circuit stays open before a trial request.
    pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

    /// Route `config` over `providers`, keyed by [`Provider::name`] as used
    /// in the config's `provider` fields.
    pub fn new(config: RouterConfig, providers: HashMap<String, Arc<dyn Provider>>) -> Self {
        for alias in config.aliases.values() {
            for c in &alias.candidates {
                if !providers.contains_key(&c.provider) {
                    tracing::warn!(
                        provider = %c.provider,
                        model = %c.model,
                        "router candidate names an unregistered provider"
                    );
                }
            }
        }
        Self {
            providers,
            pricing: config.pricing.clone(),
            router: Router::new(config),
            health: Mutex::new(HashMap::new()),
            failure_threshold: Self::DEFAULT_FAILURE_THRESHOLD,
            cooldown: Self::DEFAULT_COOLDOWN,
        }
    }

    /// Tune the circuit breaker.
    pub fn with_circuit(mut self, failure_threshold: u32, cooldown: Duration) -> Self {
        self.failure_threshold = failure_threshold.max(1);
        self.cooldown = cooldown;
        self
    }

    /// The alias router.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// The pricing table.
    pub fn pricing(&self) -> &Pricing {
        &self.pricing
    }

    /// Registered backend by name.
    pub fn provider(&self, name: &str) -> Option<&Arc<dyn Provider>> {
        self.providers.get(name)
    }

    /// Current health of a candidate.
    pub fn circuit_state(&self, candidate: &Candidate) -> CircuitState {
        let health = self.health.lock().unwrap_or_else(|p| p.into_inner());
        match health.get(&candidate.key()).and_then(|c| c.opened_at) {
            None => CircuitState::Closed,
            Some(t) if t.elapsed() >= self.cooldown => CircuitState::HalfOpen,
            Some(_) => CircuitState::Open,
        }
    }

    /// Resolve the alias and apply its defaults. Returns the candidates and
    /// the request as each backend will see it (bar `model`).
    fn prepare(
        &self,
        mut req: CompletionRequest,
    ) -> Result<(Vec<Candidate>, CompletionRequest), ProviderError> {
        let candidates = self.router.candidates(&req.alias).to_vec();
        if candidates.is_empty() {
            return Err(ProviderError::UnknownAlias(req.alias.0.clone()));
        }
        if let Some(defaults) = self.router.options(&req.alias) {
            apply_defaults(&mut req.options, defaults);
        }
        Ok((candidates, req))
    }

    /// Whether a request may go to this candidate now; a half-open circuit
    /// admits one trial and re-arms so concurrent callers wait for it.
    fn admit(&self, key: &str) -> bool {
        let mut health = self.health.lock().unwrap_or_else(|p| p.into_inner());
        let c = health.entry(key.to_string()).or_default();
        match c.opened_at {
            None => true,
            Some(t) if t.elapsed() >= self.cooldown => {
                c.opened_at = Some(Instant::now());
                true
            }
            Some(_) => false,
        }
    }

    fn record(&self, key: &str, err: Option<&ProviderError>) {
        let mut health = self.health.lock().unwrap_or_else(|p| p.into_inner());
        let c = health.entry(key.to_string()).or_default();
        match err {
            None => *c = Circuit::default(),
            Some(ProviderError::BadRequest(_)) => {}
            Some(_) => {
                c.failures += 1;
                if c.failures >= self.failure_threshold {
                    c.opened_at = Some(Instant::now());
                }
            }
        }
    }

    fn exhausted(alias: &ModelAlias, last: Option<String>, skipped: usize) -> ProviderError {
        ProviderError::Exhausted {
            alias: alias.0.clone(),
            last: last.unwrap_or_else(|| format!("{skipped} candidate(s) skipped, circuit open")),
        }
    }
}

/// Fill `cost_usd` from the table when the backend reported none. The served
/// model name is tried first, then the configured one.
fn price(pricing: &Pricing, resp: &mut CompletionResponse, configured_model: &str) {
    if resp.usage.cost_usd.is_none() {
        resp.usage.cost_usd = pricing
            .cost(&resp.model, &resp.usage)
            .or_else(|| pricing.cost(configured_model, &resp.usage));
    }
}

#[async_trait::async_trait]
impl Provider for RoutedProvider {
    fn name(&self) -> &str {
        "router"
    }

    /// The intersection of the registered backends' capabilities, except
    /// that cost is reported whenever the pricing table is non-empty.
    fn capabilities(&self) -> Capabilities {
        let mut all = self.providers.values().map(|p| p.capabilities());
        let Some(first) = all.next() else {
            return Capabilities::default();
        };
        let mut caps = all.fold(first, |a, b| Capabilities {
            streaming: a.streaming && b.streaming,
            tools: a.tools && b.tools,
            json_schema: a.json_schema && b.json_schema,
            prompt_cache: a.prompt_cache && b.prompt_cache,
            reasoning_control: a.reasoning_control && b.reasoning_control,
            cost_reported: a.cost_reported && b.cost_reported,
        });
        caps.cost_reported = caps.cost_reported || !self.pricing.is_empty();
        caps
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        let (candidates, req) = self.prepare(req)?;
        let mut last = None;
        let mut skipped = 0;
        for c in candidates {
            let key = c.key();
            let Some(p) = self.providers.get(&c.provider) else {
                last = Some(format!("provider `{}` is not registered", c.provider));
                continue;
            };
            if !self.admit(&key) {
                skipped += 1;
                continue;
            }
            let mut r = req.clone();
            r.model = c.model.clone();
            match p.complete(r).await {
                Ok(mut resp) => {
                    self.record(&key, None);
                    price(&self.pricing, &mut resp, &c.model);
                    return Ok(resp);
                }
                Err(e) => {
                    tracing::warn!(candidate = %key, error = %e, "candidate failed");
                    self.record(&key, Some(&e));
                    last = Some(e.to_string());
                }
            }
        }
        Err(Self::exhausted(&req.alias, last, skipped))
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        let (candidates, req) = self.prepare(req)?;
        let mut last = None;
        let mut skipped = 0;
        for c in candidates {
            let key = c.key();
            let Some(p) = self.providers.get(&c.provider) else {
                last = Some(format!("provider `{}` is not registered", c.provider));
                continue;
            };
            if !self.admit(&key) {
                skipped += 1;
                continue;
            }
            let mut r = req.clone();
            r.model = c.model.clone();
            match p.stream(r).await {
                Ok(s) => {
                    self.record(&key, None);
                    let pricing = self.pricing.clone();
                    let model = c.model.clone();
                    return Ok(s
                        .map(move |ev| {
                            ev.map(|ev| match ev {
                                StreamEvent::Done(mut resp) => {
                                    price(&pricing, &mut resp, &model);
                                    StreamEvent::Done(resp)
                                }
                                other => other,
                            })
                        })
                        .boxed());
                }
                Err(e) => {
                    tracing::warn!(candidate = %key, error = %e, "candidate failed to open stream");
                    self.record(&key, Some(&e));
                    last = Some(e.to_string());
                }
            }
        }
        Err(Self::exhausted(&req.alias, last, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Message, StopReason};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Answers every request and remembers what it was asked.
    struct Canned {
        calls: Mutex<Vec<CompletionRequest>>,
        cost: Option<f64>,
    }

    impl Canned {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(vec![]),
                cost: None,
            })
        }
        fn count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn last(&self) -> CompletionRequest {
            self.calls.lock().unwrap().last().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Provider for Canned {
        fn name(&self) -> &str {
            "canned"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                streaming: true,
                tools: true,
                json_schema: true,
                prompt_cache: true,
                reasoning_control: true,
                cost_reported: self.cost.is_some(),
            }
        }
        async fn complete(
            &self,
            req: CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            self.calls.lock().unwrap().push(req.clone());
            Ok(CompletionResponse {
                model: req.model,
                content: vec![ContentBlock::Text {
                    text: "canned".into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 100_000,
                    cache_read_tokens: 1_000_000,
                    cache_write_tokens: 0,
                    cost_usd: self.cost,
                },
            })
        }
    }

    /// Fails every request with the configured error.
    struct Failing {
        calls: AtomicU32,
        make: fn() -> ProviderError,
    }

    impl Failing {
        fn new(make: fn() -> ProviderError) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU32::new(0),
                make,
            })
        }
        fn count(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Provider for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn complete(
            &self,
            _req: CompletionRequest,
        ) -> Result<CompletionResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err((self.make)())
        }
    }

    fn req(alias: &str) -> CompletionRequest {
        CompletionRequest {
            alias: alias.into(),
            model: String::new(),
            system: String::new(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            output_schema: None,
            max_tokens: 16,
            options: ProviderOptions::default(),
        }
    }

    fn two_candidate_config() -> RouterConfig {
        RouterConfig::default().with_alias(
            "worker",
            vec![
                Candidate::new("a", "model-a"),
                Candidate::new("b", "model-b"),
            ],
            ProviderOptions {
                effort: Some("low".into()),
                cache_prefix: true,
                ..Default::default()
            },
        )
    }

    fn providers(a: Arc<dyn Provider>, b: Arc<dyn Provider>) -> HashMap<String, Arc<dyn Provider>> {
        [("a".to_string(), a), ("b".to_string(), b)]
            .into_iter()
            .collect()
    }

    #[test]
    fn resolves_in_order_and_unknown_is_empty() {
        let r = Router::new(two_candidate_config());
        assert_eq!(r.candidates(&ModelAlias::worker())[0].model, "model-a");
        assert!(r.candidates(&ModelAlias::judge()).is_empty());
        assert_eq!(
            r.options(&ModelAlias::worker()).unwrap().effort.as_deref(),
            Some("low")
        );
    }

    #[tokio::test]
    async fn sets_model_and_applies_alias_defaults_without_overriding() {
        let a = Canned::new();
        let routed =
            RoutedProvider::new(two_candidate_config(), providers(a.clone(), Canned::new()));
        let resp = routed.complete(req("worker")).await.unwrap();
        assert_eq!(resp.model, "model-a");
        let seen = a.last();
        assert_eq!(seen.model, "model-a");
        assert_eq!(seen.options.effort.as_deref(), Some("low"));
        assert!(seen.options.cache_prefix);

        let mut r = req("worker");
        r.options.effort = Some("max".into());
        routed.complete(r).await.unwrap();
        assert_eq!(a.last().options.effort.as_deref(), Some("max"));
    }

    #[tokio::test]
    async fn unknown_alias_is_reported() {
        let routed = RoutedProvider::new(
            two_candidate_config(),
            providers(Canned::new(), Canned::new()),
        );
        assert!(matches!(
            routed.complete(req("judge")).await,
            Err(ProviderError::UnknownAlias(a)) if a == "judge"
        ));
    }

    #[tokio::test]
    async fn fails_over_to_second_candidate_on_network_error() {
        let a = Failing::new(|| ProviderError::Network("down".into()));
        let b = Canned::new();
        let routed = RoutedProvider::new(two_candidate_config(), providers(a.clone(), b.clone()));
        let resp = routed.complete(req("worker")).await.unwrap();
        assert_eq!(resp.model, "model-b");
        assert_eq!(a.count(), 1);
        assert_eq!(b.count(), 1);
    }

    #[tokio::test]
    async fn auth_and_bad_request_still_try_the_next_candidate() {
        for make in [
            (|| ProviderError::Auth("bad key".into())) as fn() -> ProviderError,
            || ProviderError::BadRequest("bad body".into()),
        ] {
            let a = Failing::new(make);
            let b = Canned::new();
            let routed = RoutedProvider::new(two_candidate_config(), providers(a, b.clone()));
            assert_eq!(
                routed.complete(req("worker")).await.unwrap().model,
                "model-b"
            );
        }
    }

    #[tokio::test]
    async fn exhausted_when_every_candidate_fails() {
        let a = Failing::new(|| ProviderError::Network("down".into()));
        let b = Failing::new(|| ProviderError::BadRequest("schema rejected".into()));
        let routed = RoutedProvider::new(two_candidate_config(), providers(a, b));
        let err = routed.complete(req("worker")).await.unwrap_err();
        match err {
            ProviderError::Exhausted { alias, last } => {
                assert_eq!(alias, "worker");
                assert!(last.contains("schema rejected"), "{last}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn circuit_opens_after_three_failures_and_half_opens_after_cooldown() {
        let a = Failing::new(|| ProviderError::Network("down".into()));
        let b = Canned::new();
        let routed = RoutedProvider::new(two_candidate_config(), providers(a.clone(), b.clone()));
        let cand_a = Candidate::new("a", "model-a");
        for _ in 0..3 {
            routed.complete(req("worker")).await.unwrap();
        }
        assert_eq!(a.count(), 3);
        assert_eq!(routed.circuit_state(&cand_a), CircuitState::Open);
        // Open: a is skipped, b serves.
        routed.complete(req("worker")).await.unwrap();
        assert_eq!(a.count(), 3);
        assert_eq!(b.count(), 4);
        assert_eq!(
            routed.circuit_state(&Candidate::new("b", "model-b")),
            CircuitState::Closed
        );

        // With no cooldown the circuit is half-open at once and a gets a trial.
        let a2 = Failing::new(|| ProviderError::Network("down".into()));
        let routed =
            RoutedProvider::new(two_candidate_config(), providers(a2.clone(), Canned::new()))
                .with_circuit(3, Duration::ZERO);
        for _ in 0..5 {
            routed.complete(req("worker")).await.unwrap();
        }
        assert_eq!(a2.count(), 5);
        assert_eq!(routed.circuit_state(&cand_a), CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn bad_requests_do_not_trip_the_circuit() {
        let a = Failing::new(|| ProviderError::BadRequest("nope".into()));
        let routed =
            RoutedProvider::new(two_candidate_config(), providers(a.clone(), Canned::new()));
        for _ in 0..5 {
            routed.complete(req("worker")).await.unwrap();
        }
        assert_eq!(a.count(), 5);
        assert_eq!(
            routed.circuit_state(&Candidate::new("a", "model-a")),
            CircuitState::Closed
        );
    }

    #[tokio::test]
    async fn success_resets_the_failure_count() {
        let a = Failing::new(|| ProviderError::Network("down".into()));
        let b = Canned::new();
        let mut cfg = two_candidate_config();
        cfg = cfg.with_alias(
            "direct",
            vec![Candidate::new("b", "model-b")],
            ProviderOptions::default(),
        );
        let routed = RoutedProvider::new(cfg, providers(a, b.clone()));
        routed.complete(req("worker")).await.unwrap();
        routed.complete(req("worker")).await.unwrap();
        // Two failures on a; a success on b resets b only, a keeps counting.
        routed.complete(req("direct")).await.unwrap();
        routed.complete(req("worker")).await.unwrap();
        assert_eq!(
            routed.circuit_state(&Candidate::new("a", "model-a")),
            CircuitState::Open
        );
    }

    #[tokio::test]
    async fn pricing_fills_cost_only_when_provider_reported_none() {
        let mut cfg = two_candidate_config();
        cfg.pricing.insert(
            "model-a",
            ModelPricing {
                input_per_mtok: 2.0,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 0.2,
                cache_write_per_mtok: 0.0,
            },
        );
        let a = Canned::new();
        let routed = RoutedProvider::new(cfg.clone(), providers(a, Canned::new()));
        let resp = routed.complete(req("worker")).await.unwrap();
        // 1M input at $2 + 100k output at $10 + 1M cache read at $0.2.
        assert!((resp.usage.cost_usd.unwrap() - 3.2).abs() < 1e-9);
        assert!(routed.capabilities().cost_reported);

        let reported = Arc::new(Canned {
            calls: Mutex::new(vec![]),
            cost: Some(0.5),
        });
        let routed = RoutedProvider::new(cfg, providers(reported, Canned::new()));
        let resp = routed.complete(req("worker")).await.unwrap();
        assert_eq!(resp.usage.cost_usd, Some(0.5));

        // Unpriced model: stays None.
        let routed = RoutedProvider::new(
            two_candidate_config(),
            providers(Canned::new(), Canned::new()),
        );
        assert_eq!(
            routed.complete(req("worker")).await.unwrap().usage.cost_usd,
            None
        );
        assert!(!routed.capabilities().cost_reported);
    }

    #[tokio::test]
    async fn stream_fails_over_and_prices_the_done_event() {
        let mut cfg = two_candidate_config();
        cfg.pricing.insert(
            "model-b",
            ModelPricing {
                input_per_mtok: 1.0,
                ..Default::default()
            },
        );
        let a = Failing::new(|| ProviderError::Network("down".into()));
        let routed = RoutedProvider::new(cfg, providers(a, Canned::new()));
        let events: Vec<_> = routed
            .stream(req("worker"))
            .await
            .unwrap()
            .map(|e| e.unwrap())
            .collect()
            .await;
        let StreamEvent::Done(resp) = &events[0] else {
            panic!("expected Done");
        };
        assert_eq!(resp.model, "model-b");
        assert!((resp.usage.cost_usd.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn parses_the_documented_toml_and_round_trips() {
        let text = r#"
[aliases.root]
candidates = [{ provider = "anthropic", model = "claude-opus-5" }]
options = { effort = "high" }

[aliases.worker]
candidates = [
  { provider = "anthropic", model = "claude-sonnet-5" },
  { provider = "openai_compat", model = "gpt-5.4-mini" },
]
options = { effort = "low" }

[pricing."claude-opus-5"]
input_per_mtok = 5.0
output_per_mtok = 25.0
cache_read_per_mtok = 0.5
"#;
        let cfg = RouterConfig::from_toml(text).unwrap();
        let worker = &cfg.aliases["worker"];
        assert_eq!(worker.candidates.len(), 2);
        assert_eq!(worker.candidates[1].provider, "openai_compat");
        assert_eq!(worker.options.effort.as_deref(), Some("low"));
        assert!(!worker.options.cache_prefix);
        assert_eq!(cfg.aliases["root"].options.effort.as_deref(), Some("high"));
        let opus = cfg.pricing.get("claude-opus-5").unwrap();
        assert_eq!(opus.input_per_mtok, 5.0);
        assert_eq!(opus.cache_read_per_mtok, 0.5);
        assert_eq!(opus.cache_write_per_mtok, 0.0);

        let again = RouterConfig::from_toml(&cfg.to_toml().unwrap()).unwrap();
        assert_eq!(again, cfg);

        let full = RouterConfig::default_for_anthropic();
        let again = RouterConfig::from_toml(&full.to_toml().unwrap()).unwrap();
        assert_eq!(again, full);
    }

    #[test]
    fn bad_toml_is_an_error() {
        assert!(matches!(
            RouterConfig::from_toml("[aliases.root]\ncandidates = 3"),
            Err(ProviderError::Other(m)) if m.starts_with("router config:")
        ));
    }

    #[test]
    fn anthropic_defaults_cover_every_alias_with_pricing() {
        let cfg = RouterConfig::default_for_anthropic();
        let r = Router::new(cfg.clone());
        for (alias, model) in [
            ("root", "claude-opus-5"),
            ("worker", "claude-sonnet-5"),
            ("proxy", "claude-haiku-4-5"),
            ("judge", "claude-sonnet-5"),
        ] {
            let c = &r.candidates(&alias.into())[0];
            assert_eq!(c.provider, "anthropic");
            assert_eq!(c.model, model);
            assert!(cfg.pricing.get(model).is_some(), "{model} unpriced");
        }
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            ..Default::default()
        };
        assert!((cfg.pricing.cost("claude-opus-5", &usage).unwrap() - 30.5).abs() < 1e-9);
        assert!((cfg.pricing.cost("claude-sonnet-5", &usage).unwrap() - 12.2).abs() < 1e-9);
        assert!((cfg.pricing.cost("claude-haiku-4-5", &usage).unwrap() - 6.1).abs() < 1e-9);
        assert_eq!(cfg.pricing.cost("gpt-5.4-mini", &usage), None);
        assert_eq!(
            r.options(&"root".into()).unwrap().effort.as_deref(),
            Some("high")
        );

        let oa = RouterConfig::default_for_openai_compat("local");
        assert_eq!(oa.aliases.len(), 4);
        assert_eq!(oa.aliases["proxy"].candidates[0].provider, "openai_compat");
    }
}
