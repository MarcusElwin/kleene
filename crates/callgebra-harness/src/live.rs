//! The live [`CallSink`]: model calls, prompt-defined functions, tools, memo,
//! budget and trace, over the store-backed sink.

use crate::sink::StoreSink;
use crate::AgentRole;
use callgebra_core::{
    Batch, Budget, BudgetUsage, CallId, CallKind, Catalog, DataType, Field, FunctionDef,
    FunctionReturn, ModelAlias, RunId, Schema, SessionId, StatementId, Value, Volatility,
};
use callgebra_exec::{BatchStream, CallSink, ExecError};
use callgebra_llm::{CompletionRequest, CompletionResponse, Message, Provider, ProviderOptions};
use callgebra_sql::{FunctionBody, Statement};
use callgebra_store::MemoEntry;
use callgebra_tools::{Tool, ToolContext, ToolRegistry};
use callgebra_trace::{TraceEvent, Tracer};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// Per-session model settings, changed with `SET`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelSettings {
    /// Effort for calls that do not specify one.
    pub effort: Option<String>,
    /// Alias used by `llm(...)` when none is given.
    pub default_alias: ModelAlias,
    /// Output cap per call.
    pub max_tokens: u32,
    /// Ask providers to cache the system prefix.
    pub cache_prefix: bool,
    /// Skip the memo (every call goes to the provider).
    pub no_memo: bool,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            effort: None,
            default_alias: ModelAlias::worker(),
            max_tokens: 1024,
            cache_prefix: true,
            no_memo: false,
        }
    }
}

/// A prompt-, SQL- or shell-defined function.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DefinedFunction {
    /// Argument names, in order.
    pub arg_names: Vec<String>,
    /// Argument types, in order.
    pub arg_types: Vec<DataType>,
    /// Return type.
    pub returns: DataType,
    /// Body.
    pub body: FunctionBody,
    /// Declared volatility.
    pub volatility: Volatility,
}

/// The catalog entry for a defined function.
pub fn function_entry(name: &str, def: &DefinedFunction) -> FunctionDef {
    let call_kind = match def.body {
        FunctionBody::Prompt { .. } => CallKind::LlmScalar {
            alias: ModelAlias::worker(),
        },
        FunctionBody::Sql { .. } => CallKind::Pure,
        FunctionBody::Shell { .. } => CallKind::Tool {
            tool: "shell".into(),
        },
    };
    FunctionDef {
        name: name.to_ascii_lowercase(),
        args: def.arg_types.clone(),
        variadic: false,
        returns: FunctionReturn::Scalar {
            data_type: def.returns,
        },
        call_kind,
        volatility: def.volatility,
        description: match &def.body {
            FunctionBody::Prompt { template } => format!("prompt: {}", first_line(template)),
            FunctionBody::Sql { query } => format!("sql: {}", first_line(query)),
            FunctionBody::Shell { command } => format!("shell: {}", first_line(command)),
        },
    }
}

/// Which session a sink serves, for trace attribution and delegation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMeta {
    /// Session id.
    pub id: SessionId,
    /// Run the session belongs to.
    pub run: RunId,
    /// Depth; root is 0.
    pub depth: u32,
    /// The role the session runs as.
    pub role: AgentRole,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            id: SessionId::new(),
            run: RunId::new(),
            depth: 0,
            role: AgentRole::root(),
        }
    }
}

/// Runs child sessions for `rlm(...)` and `spawn(...)`; implemented by the
/// harness, injected into the sink so the executor never sees sessions.
#[async_trait::async_trait]
pub trait ChildRunner: Send + Sync {
    /// Run one child of `parent`. `agent` is `None` for `rlm` (the parent's
    /// own role one level deeper) or a declared agent name for `spawn`.
    /// Returns the child's `FINAL` relation.
    async fn run_child(
        &self,
        parent: &LiveSink,
        agent: Option<&str>,
        task: String,
        context: Option<String>,
    ) -> Result<Batch, ExecError>;
}

/// Budget state shared by every call of a session.
#[derive(Debug, Default)]
pub struct BudgetState {
    /// Limits.
    pub budget: callgebra_core::Budget,
    /// Spent so far.
    pub usage: BudgetUsage,
    /// Calls served from the memo (free, not counted in `usage`).
    pub memo_hits: u64,
}

/// Everything a statement's calls need.
pub struct LiveSink {
    store: StoreSink,
    provider: Option<Arc<dyn Provider>>,
    tools: Arc<ToolRegistry>,
    tool_ctx: ToolContext,
    tracer: Option<Tracer>,
    settings: RwLock<ModelSettings>,
    functions: RwLock<HashMap<String, DefinedFunction>>,
    budget: Mutex<BudgetState>,
    /// The statement currently executing, for trace attribution.
    statement: Mutex<Option<StatementId>>,
    /// System prefix for model calls (frozen per session for caching).
    system: RwLock<String>,
    meta: RwLock<SessionMeta>,
    agents: RwLock<HashMap<String, AgentRole>>,
    runner: RwLock<Option<Arc<dyn ChildRunner>>>,
}

const SYSTEM_PREFIX: &str = "You are a function inside a SQL engine. Answer only with the value asked for: no preamble, no explanation, no markdown fences.";

impl LiveSink {
    /// Build over a store-backed sink. `provider` may be `None` (model calls
    /// then fail with a clear message); `tools` decides what `CALL` can run.
    pub fn new(
        store: StoreSink,
        provider: Option<Arc<dyn Provider>>,
        tools: Arc<ToolRegistry>,
        tool_ctx: ToolContext,
        tracer: Option<Tracer>,
    ) -> Self {
        Self {
            store,
            provider,
            tools,
            tool_ctx,
            tracer,
            settings: RwLock::new(ModelSettings::default()),
            functions: RwLock::new(HashMap::new()),
            budget: Mutex::new(BudgetState::default()),
            statement: Mutex::new(None),
            system: RwLock::new(SYSTEM_PREFIX.to_string()),
            meta: RwLock::new(SessionMeta::default()),
            agents: RwLock::new(HashMap::new()),
            runner: RwLock::new(None),
        }
    }

    /// Which session this sink serves.
    pub async fn meta(&self) -> SessionMeta {
        self.meta.read().await.clone()
    }

    /// Set the session this sink serves.
    pub async fn set_meta(&self, meta: SessionMeta) {
        *self.meta.write().await = meta;
    }

    /// Install the runner that `rlm(...)` and `spawn(...)` delegate to.
    pub async fn set_child_runner(&self, runner: Arc<dyn ChildRunner>) {
        *self.runner.write().await = Some(runner);
    }

    /// The provider, if any (children share their parent's).
    pub fn provider(&self) -> Option<Arc<dyn Provider>> {
        self.provider.clone()
    }

    /// The tool registry.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.tools.clone()
    }

    /// The tool context.
    pub fn tool_ctx(&self) -> &ToolContext {
        &self.tool_ctx
    }

    /// The tracer, if any.
    pub fn tracer(&self) -> Option<Tracer> {
        self.tracer.clone()
    }

    /// Replace the budget limits (children get a slice of their parent's).
    pub async fn set_budget(&self, budget: Budget) {
        self.budget.lock().await.budget = budget;
    }

    /// Replace the settings (children inherit their parent's).
    pub async fn set_settings(&self, settings: ModelSettings) {
        *self.settings.write().await = settings;
    }

    /// Count a finished child's spending against this session.
    pub async fn charge_child(&self, usage: &BudgetUsage) {
        let mut b = self.budget.lock().await;
        b.usage.calls += usage.calls;
        b.usage.tokens += usage.tokens;
        b.usage.dollars += usage.dollars;
    }

    /// Declared agent roles, by lower-case name.
    pub async fn agents(&self) -> HashMap<String, AgentRole> {
        self.agents.read().await.clone()
    }

    /// Replace the declared agents (children inherit their parent's).
    pub async fn set_agents(&self, agents: HashMap<String, AgentRole>) {
        *self.agents.write().await = agents;
    }

    /// Prompt-, SQL- and shell-defined functions, by lower-case name.
    pub async fn functions(&self) -> HashMap<String, DefinedFunction> {
        self.functions.read().await.clone()
    }

    /// Replace the defined functions (children inherit their parent's; the
    /// catalog entries travel with the base catalog).
    pub async fn set_functions(&self, functions: HashMap<String, DefinedFunction>) {
        *self.functions.write().await = functions;
    }

    /// Declare an agent role from a `CREATE AGENT` statement.
    pub async fn define_agent(&self, stmt: &Statement) -> Result<String, ExecError> {
        let callgebra_sql::StatementKind::CreateAgent {
            name,
            model,
            effort,
            tools,
            budget,
            prompt,
            replace,
        } = &stmt.kind
        else {
            return Err(ExecError::Eval("not a CREATE AGENT".into()));
        };
        let key = name.to_ascii_lowercase();
        if key == "self" || key == "root" {
            return Err(ExecError::Eval(format!("{name} is a reserved role name")));
        }
        if !replace && self.agents.read().await.contains_key(&key) {
            return Err(ExecError::Eval(format!(
                "agent {name} already exists; use CREATE OR REPLACE AGENT"
            )));
        }
        let mut b = Budget::unbounded();
        for (dim, v) in budget {
            match dim.as_str() {
                "calls" => b.calls = Some(*v as u64),
                "tokens" => b.tokens = Some(*v as u64),
                "dollars" => b.dollars = Some(*v),
                "depth" => b.max_depth = Some(*v as u32),
                "turns" => b.wall = None,
                _ => {}
            }
        }
        let turns = budget
            .iter()
            .find(|(d, _)| d == "turns")
            .map(|(_, v)| *v as u32);
        let role = AgentRole {
            name: key.clone(),
            model: ModelAlias(model.clone()),
            effort: effort.clone(),
            tools: tools.clone(),
            budget: b,
            prompt: prompt.clone(),
            max_turns: turns,
        };
        self.agents.write().await.insert(key, role);
        Ok(format!("defined agent {name}"))
    }

    /// The store-backed sink underneath.
    pub fn store(&self) -> &StoreSink {
        &self.store
    }

    /// The catalog handle.
    pub fn catalog(&self) -> Arc<RwLock<Catalog>> {
        self.store.catalog()
    }

    /// Register the tools' catalog entries so the planner resolves them.
    pub async fn register_tool_catalog(&self, entries: Vec<FunctionDef>) {
        let catalog = self.store.catalog();
        let mut cat = catalog.write().await;
        for e in entries {
            cat.add_function(e);
        }
    }

    /// Which statement is running (for trace attribution).
    pub async fn set_statement(&self, id: Option<StatementId>) {
        *self.statement.lock().await = id;
    }

    /// Current settings.
    pub async fn settings(&self) -> ModelSettings {
        self.settings.read().await.clone()
    }

    /// Apply a `SET key = value`. Returns a message for the transcript.
    pub async fn apply_set(&self, key: &str, value: &str) -> Result<String, ExecError> {
        let lower = key.to_ascii_lowercase();
        let bad = |what: &str| ExecError::Eval(format!("SET {key}: {what}"));
        match lower.as_str() {
            "effort" => {
                self.settings.write().await.effort = Some(value.to_string());
            }
            "model.default" | "model" => {
                self.settings.write().await.default_alias = ModelAlias(value.to_string());
            }
            "max_tokens" => {
                self.settings.write().await.max_tokens = value.parse().map_err(|_| bad("expects an integer"))?;
            }
            "memo" => {
                self.settings.write().await.no_memo = matches!(value.to_ascii_lowercase().as_str(), "off" | "false" | "0");
            }
            "cache_prefix" => {
                self.settings.write().await.cache_prefix = !matches!(value.to_ascii_lowercase().as_str(), "off" | "false" | "0");
            }
            "budget.calls" => {
                self.budget.lock().await.budget.calls = Some(value.parse().map_err(|_| bad("expects an integer"))?);
            }
            "budget.tokens" => {
                self.budget.lock().await.budget.tokens = Some(value.parse().map_err(|_| bad("expects an integer"))?);
            }
            "budget.dollars" => {
                self.budget.lock().await.budget.dollars = Some(value.parse().map_err(|_| bad("expects a number"))?);
            }
            "budget.depth" => {
                self.budget.lock().await.budget.max_depth = Some(value.parse().map_err(|_| bad("expects an integer"))?);
            }
            other => {
                return Err(ExecError::Eval(format!(
                    "unknown setting {other}; known: effort, model.default, max_tokens, memo, cache_prefix, budget.calls, budget.tokens, budget.dollars, budget.depth"
                )))
            }
        }
        Ok(format!("set {key} = {value}"))
    }

    /// Usage so far.
    pub async fn usage(&self) -> BudgetUsage {
        self.budget.lock().await.usage
    }

    /// Limits and usage (statement-level accounting is done by the caller diffing).
    pub async fn budget_state(&self) -> (callgebra_core::Budget, BudgetUsage) {
        let b = self.budget.lock().await;
        (b.budget.clone(), b.usage)
    }

    /// Calls served from the memo so far; they cost nothing and do not count
    /// against `budget.calls`.
    pub async fn memo_hits(&self) -> u64 {
        self.budget.lock().await.memo_hits
    }

    /// Define a function from a `CREATE FUNCTION` statement: registers it in
    /// the catalog and remembers its body.
    pub async fn define_function(&self, stmt: &Statement) -> Result<String, ExecError> {
        let callgebra_sql::StatementKind::CreateFunction {
            name,
            args,
            returns,
            body,
            volatility,
            replace,
        } = &stmt.kind
        else {
            return Err(ExecError::Eval("not a CREATE FUNCTION".into()));
        };
        {
            let catalog = self.store.catalog();
            let cat = catalog.read().await;
            if let Some(existing) = cat.function(name) {
                if !replace {
                    return Err(ExecError::Eval(format!(
                        "function {name} already exists; use CREATE OR REPLACE FUNCTION"
                    )));
                }
                if existing.call_kind == CallKind::Pure
                    && callgebra_core::standard_functions()
                        .iter()
                        .any(|f| f.name.eq_ignore_ascii_case(name))
                {
                    return Err(ExecError::Eval(format!(
                        "cannot replace the builtin {name}"
                    )));
                }
            }
        }
        let defined = DefinedFunction {
            arg_names: args.iter().map(|(n, _)| n.clone()).collect(),
            arg_types: args.iter().map(|(_, t)| *t).collect(),
            returns: *returns,
            body: body.clone(),
            volatility: *volatility,
        };
        let lower = name.to_ascii_lowercase();
        self.store
            .catalog()
            .write()
            .await
            .add_function(function_entry(&lower, &defined));
        self.functions.write().await.insert(lower, defined);
        Ok(format!("defined function {name}"))
    }

    /// Charge tokens and dollars for a call whose slot `reserve_call` took.
    async fn charge(&self, usage: &callgebra_llm::Usage) -> Result<(), ExecError> {
        let mut b = self.budget.lock().await;
        b.usage.tokens += usage.total_tokens();
        b.usage.dollars += usage.cost_usd.unwrap_or(0.0);
        let snapshot = b.usage;
        b.budget.check(&snapshot).map_err(ExecError::Budget)
    }

    /// Take one call slot from the budget before the provider is asked, so
    /// concurrent rows cannot overshoot `budget.calls`. A refused call is not
    /// counted.
    async fn reserve_call(&self) -> Result<(), ExecError> {
        let mut b = self.budget.lock().await;
        let mut next = b.usage;
        next.calls += 1;
        b.budget.check(&next).map_err(ExecError::Budget)?;
        b.usage.calls = next.calls;
        Ok(())
    }

    /// Build a request in this session's settings: system prefix, one user
    /// message, optional JSON schema and effort override.
    pub async fn request(
        &self,
        alias: ModelAlias,
        prompt: String,
        output_schema: Option<serde_json::Value>,
        effort: Option<String>,
    ) -> CompletionRequest {
        let settings = self.settings().await;
        CompletionRequest {
            alias,
            model: String::new(),
            system: self.system.read().await.clone(),
            messages: vec![Message::user(prompt)],
            tools: vec![],
            output_schema,
            max_tokens: settings.max_tokens,
            options: ProviderOptions {
                effort: effort.or(settings.effort.clone()),
                cache_prefix: settings.cache_prefix,
                temperature: None,
                extra: Default::default(),
            },
        }
    }

    /// Run one request through memo, provider, budget and trace. This is the
    /// single path every model call takes, statement calls and session turns
    /// alike.
    pub async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ExecError> {
        let provider = self.provider.as_ref().ok_or_else(|| {
            ExecError::Call(
                "no model provider configured (set ANTHROPIC_API_KEY or OPENAI_API_KEY, or a CALLGEBRA_ROUTER_TOML)".into(),
            )
        })?;
        let settings = self.settings().await;
        let alias = req.alias.clone();
        let fingerprint = req.fingerprint();
        let call_id = CallId::new();
        let statement = self.statement.lock().await.unwrap_or_default();
        let started = Instant::now();
        if let Some(t) = &self.tracer {
            t.emit(TraceEvent::CallStarted {
                statement,
                call: call_id,
                alias: alias.0.clone(),
                model: String::new(),
                fingerprint: fingerprint.clone(),
            });
        }
        // Memo: keyed by alias + fingerprint (the alias stands in for the model
        // until the router reports which one served).
        if !settings.no_memo {
            if let Ok(Some(hit)) = self.store.store().memo_get(&alias.0, &fingerprint).await {
                let resp: CompletionResponse =
                    match serde_json::from_value::<CompletionResponse>(hit.response.clone()) {
                        Ok(r) => r,
                        Err(_) => CompletionResponse {
                            model: hit
                                .response
                                .get("model")
                                .and_then(|m| m.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            content: vec![callgebra_llm::ContentBlock::Text {
                                text: hit
                                    .response
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            }],
                            stop_reason: callgebra_llm::StopReason::EndTurn,
                            usage: callgebra_llm::Usage::default(),
                        },
                    };
                self.budget.lock().await.memo_hits += 1;
                if let Some(t) = &self.tracer {
                    t.emit(TraceEvent::CallFinished {
                        call: call_id,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cost_usd: 0.0,
                        elapsed: started.elapsed(),
                        memo_hit: true,
                        error: None,
                    });
                }
                return Ok(resp);
            }
        }
        self.reserve_call().await?;
        match provider.complete(req).await {
            Ok(resp) => {
                self.charge(&resp.usage).await?;
                if let Some(t) = &self.tracer {
                    t.emit(TraceEvent::CallFinished {
                        call: call_id,
                        input_tokens: resp.usage.input_tokens,
                        output_tokens: resp.usage.output_tokens,
                        cache_read_tokens: resp.usage.cache_read_tokens,
                        cost_usd: resp.usage.cost_usd.unwrap_or(0.0),
                        elapsed: started.elapsed(),
                        memo_hit: false,
                        error: (resp.stop_reason == callgebra_llm::StopReason::Refusal)
                            .then(|| "the model declined this request".to_string()),
                    });
                }
                if !settings.no_memo && resp.stop_reason != callgebra_llm::StopReason::Refusal {
                    let entry = MemoEntry {
                        response: serde_json::to_value(&resp).unwrap_or_default(),
                        input_tokens: resp.usage.input_tokens,
                        output_tokens: resp.usage.output_tokens,
                        cost_usd: resp.usage.cost_usd,
                    };
                    let _ = self
                        .store
                        .store()
                        .memo_put(&alias.0, &fingerprint, &entry)
                        .await;
                }
                Ok(resp)
            }
            Err(e) => {
                if let Some(t) = &self.tracer {
                    t.emit(TraceEvent::CallFinished {
                        call: call_id,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cost_usd: 0.0,
                        elapsed: started.elapsed(),
                        memo_hit: false,
                        error: Some(e.to_string()),
                    });
                }
                Err(ExecError::Call(e.to_string()))
            }
        }
    }

    /// One scalar model call: the response text, or an error if the model
    /// declined.
    async fn model_call(
        &self,
        alias: ModelAlias,
        prompt: String,
        output_schema: Option<serde_json::Value>,
        effort: Option<String>,
    ) -> Result<String, ExecError> {
        let req = self.request(alias, prompt, output_schema, effort).await;
        let resp = self.complete(req).await?;
        if resp.stop_reason == callgebra_llm::StopReason::Refusal {
            return Err(ExecError::Call("the model declined this request".into()));
        }
        Ok(resp.text())
    }

    fn bool_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "boolean" } },
            "required": ["answer"],
            "additionalProperties": false
        })
    }

    fn parse_bool(text: &str) -> Result<Value, ExecError> {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(b) = v
                .get("answer")
                .and_then(|a| a.as_bool())
                .or_else(|| v.as_bool())
            {
                return Ok(Value::Bool(b));
            }
        }
        let t = text
            .trim()
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        Ok(match t.as_str() {
            "yes" | "true" | "y" => Value::Bool(true),
            "no" | "false" | "n" => Value::Bool(false),
            _ => {
                if t.starts_with("yes") || t.starts_with("true") {
                    Value::Bool(true)
                } else if t.starts_with("no") || t.starts_with("false") {
                    Value::Bool(false)
                } else {
                    return Err(ExecError::Call(format!(
                        "expected yes/no, got: {}",
                        text.trim()
                    )));
                }
            }
        })
    }

    async fn run_defined(
        &self,
        name: &str,
        def: DefinedFunction,
        args: &[Value],
    ) -> Result<Value, ExecError> {
        if args.len() != def.arg_names.len() {
            return Err(ExecError::Call(format!(
                "{name} takes {} arguments, got {}",
                def.arg_names.len(),
                args.len()
            )));
        }
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        match &def.body {
            FunctionBody::Prompt { template } => {
                let prompt = substitute(template, &def.arg_names, args);
                let (schema, prompt) = match def.returns {
                    DataType::Bool => (
                        Some(Self::bool_schema()),
                        format!("{prompt}\n\nAnswer with JSON: {{\"answer\": true}} or {{\"answer\": false}}."),
                    ),
                    DataType::Json => (None, format!("{prompt}\n\nAnswer with JSON only.")),
                    _ => (None, prompt),
                };
                let text = self
                    .model_call(self.settings().await.default_alias, prompt, schema, None)
                    .await?;
                coerce_text(&text, def.returns)
            }
            FunctionBody::Shell { command } => {
                let cmd = substitute(command, &def.arg_names, args);
                let tool = self
                    .tools
                    .get("shell")
                    .ok_or_else(|| ExecError::Call("shell tool not available".into()))?;
                let batch = tool
                    .call(&[Value::Text(cmd)], &self.tool_ctx)
                    .await
                    .map_err(|e| ExecError::Call(e.to_string()))?;
                let row = batch
                    .rows
                    .first()
                    .ok_or_else(|| ExecError::Call("shell returned no row".into()))?;
                let stdout = row.first().map(Value::render).unwrap_or_default();
                let exit = batch
                    .schema
                    .index_of("exit_code")
                    .and_then(|i| row[i].as_int())
                    .unwrap_or(0);
                match def.returns {
                    DataType::Bool => Ok(Value::Bool(exit == 0)),
                    other => coerce_text(stdout.trim(), other),
                }
            }
            FunctionBody::Sql { .. } => Err(ExecError::Call(format!(
                "{name}: SQL-defined functions are inlined by the planner in M5; not callable yet"
            ))),
        }
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or_default();
    if l.chars().count() > 60 {
        format!("{}…", l.chars().take(60).collect::<String>())
    } else {
        l.to_string()
    }
}

/// Replace `{name}` placeholders with rendered argument values.
pub fn substitute(template: &str, names: &[String], args: &[Value]) -> String {
    let mut out = template.to_string();
    for (n, v) in names.iter().zip(args) {
        out = out.replace(&format!("{{{n}}}"), &v.render());
    }
    out
}

fn coerce_text(text: &str, to: DataType) -> Result<Value, ExecError> {
    let t = text.trim();
    Ok(match to {
        DataType::Text | DataType::Any => Value::Text(t.to_string()),
        DataType::Bool => LiveSink::parse_bool(t)?,
        DataType::Json => Value::Json(
            serde_json::from_str(t)
                .map_err(|e| ExecError::Call(format!("expected JSON, got: {t} ({e})")))?,
        ),
        DataType::Int => Value::Int(
            t.parse()
                .map_err(|_| ExecError::Call(format!("expected an integer, got: {t}")))?,
        ),
        DataType::Float => Value::Float(
            t.parse()
                .map_err(|_| ExecError::Call(format!("expected a number, got: {t}")))?,
        ),
        DataType::Vector => Err(ExecError::Call("vector results are not supported".into()))?,
    })
}

#[async_trait::async_trait]
impl CallSink for LiveSink {
    async fn scalar_call(&self, name: &str, args: &[Value]) -> Result<Value, ExecError> {
        let lower = name.to_ascii_lowercase();
        if let Some(def) = self.functions.read().await.get(&lower).cloned() {
            return self.run_defined(&lower, def, args).await;
        }
        match lower.as_str() {
            "llm" => {
                let Some(prompt) = args.first().and_then(|v| v.as_text()) else {
                    return Ok(Value::Null);
                };
                let alias = args
                    .get(1)
                    .and_then(|v| v.as_text())
                    .map(|a| ModelAlias(a.to_string()))
                    .unwrap_or(self.settings().await.default_alias);
                let effort = args.get(2).and_then(|v| v.as_text()).map(str::to_string);
                self.model_call(alias, prompt.to_string(), None, effort)
                    .await
                    .map(Value::Text)
            }
            "llm_bool" => {
                let Some(prompt) = args.first().and_then(|v| v.as_text()) else {
                    return Ok(Value::Null);
                };
                let prompt = format!(
                    "{prompt}\n\nAnswer with JSON: {{\"answer\": true}} or {{\"answer\": false}}."
                );
                let text = self
                    .model_call(
                        self.settings().await.default_alias,
                        prompt,
                        Some(Self::bool_schema()),
                        None,
                    )
                    .await?;
                Self::parse_bool(&text)
            }
            "llm_json" => {
                let (Some(prompt), Some(schema)) = (
                    args.first().and_then(|v| v.as_text()),
                    args.get(1).and_then(|v| v.as_text()),
                ) else {
                    return Ok(Value::Null);
                };
                let schema: serde_json::Value = serde_json::from_str(schema)
                    .map_err(|e| ExecError::Call(format!("llm_json: schema is not JSON: {e}")))?;
                let text = self
                    .model_call(
                        self.settings().await.default_alias,
                        format!("{prompt}\n\nAnswer with JSON only."),
                        Some(schema),
                        None,
                    )
                    .await?;
                coerce_text(&text, DataType::Json)
            }
            other => Err(ExecError::Call(format!(
                "no implementation for function {other}"
            ))),
        }
    }

    async fn table_call(&self, name: &str, args: &[Value]) -> Result<Batch, ExecError> {
        let lower = name.to_ascii_lowercase();
        if lower == "rlm" || lower == "spawn" {
            let runner = self.runner.read().await.clone().ok_or_else(|| {
                ExecError::Call(format!(
                    "{lower} needs a running harness (callgebra run); the plain REPL cannot spawn sessions"
                ))
            })?;
            let text = |i: usize| args.get(i).and_then(|v| v.as_text()).map(str::to_string);
            let (agent, task, context, max) = if lower == "rlm" {
                (None, text(0), text(1), 2)
            } else {
                (text(0), text(1), text(2), 3)
            };
            if args.len() > max {
                return Err(ExecError::Call(format!(
                    "{lower} takes at most {max} arguments, got {}",
                    args.len()
                )));
            }
            let Some(task) = task else {
                return Err(ExecError::Call(format!("{lower}: the task must be text")));
            };
            if lower == "spawn" && agent.is_none() {
                return Err(ExecError::Call("spawn: the agent name must be text".into()));
            }
            return runner
                .run_child(self, agent.as_deref(), task, context)
                .await;
        }
        if lower == "expand" {
            let (Some(prompt), Some(n)) = (
                args.first().and_then(|v| v.as_text()),
                args.get(1).and_then(|v| v.as_int()),
            ) else {
                return Ok(Batch::empty(Arc::new(Schema::new(vec![Field::not_null(
                    "item",
                    DataType::Text,
                )]))));
            };
            let prompt = format!(
                "{prompt}\n\nReturn at most {n} items as a JSON array of strings, nothing else."
            );
            let schema =
                serde_json::json!({"type": "array", "items": {"type": "string"}, "maxItems": n});
            let text = self
                .model_call(
                    self.settings().await.default_alias,
                    prompt,
                    Some(schema),
                    None,
                )
                .await?;
            let items: Vec<String> = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(serde_json::Value::Array(a)) => a
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect(),
                Ok(serde_json::Value::Object(o)) => o
                    .values()
                    .find_map(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => text
                    .lines()
                    .map(|l| l.trim().trim_start_matches(['-', '*', ' ']).to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
            };
            let schema = Arc::new(Schema::new(vec![Field::not_null("item", DataType::Text)]));
            let rows = items
                .into_iter()
                .take(n.max(0) as usize)
                .map(|s| vec![Value::Text(s)])
                .collect();
            return Batch::try_new(schema, rows).map_err(ExecError::from);
        }
        let Some(tool) = self.tools.get(&lower) else {
            return Err(ExecError::Call(format!(
                "no implementation for table function {name}"
            )));
        };
        let statement = self.statement.lock().await.unwrap_or_default();
        let started = Instant::now();
        let result = tool.call(args, &self.tool_ctx).await;
        if let Some(t) = &self.tracer {
            t.emit(TraceEvent::ToolCall {
                statement,
                tool: lower.clone(),
                args: args
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", "),
                bytes_out: result
                    .as_ref()
                    .map(|b| {
                        b.rows
                            .iter()
                            .flatten()
                            .map(|v| v.render().len() as u64)
                            .sum()
                    })
                    .unwrap_or(0),
                elapsed: started.elapsed(),
                error: result.as_ref().err().map(|e| e.to_string()),
            });
        }
        result.map_err(|e| ExecError::Call(e.to_string()))
    }

    async fn scan(&self, table: &str) -> Result<BatchStream, ExecError> {
        self.store.scan(table).await
    }

    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        self.store.create_table(name, schema, if_not_exists).await
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), ExecError> {
        self.store.insert(name, batch).await
    }

    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError> {
        self.store.drop_table(name, if_exists).await
    }
}

/// Catalog entries for tools, from a registry, so the planner can resolve
/// them (the tools crate provides the definitive builder; this adapts any
/// registry to `FunctionDef`s using each tool's schema and volatility).
pub fn tool_catalog_entries(tools: &ToolRegistry) -> Vec<FunctionDef> {
    tools
        .iter()
        .map(|t: &Arc<dyn Tool>| FunctionDef {
            name: t.name().to_string(),
            args: vec![DataType::Any],
            variadic: true,
            returns: FunctionReturn::Table {
                schema: (*t.schema()).clone(),
            },
            call_kind: CallKind::Tool {
                tool: t.name().to_string(),
            },
            volatility: t.volatility(),
            description: t.description().to_string(),
        })
        .collect()
}

/// Whether a function definition is a side effect that only `CALL` may run.
pub fn is_side_effect(def: &FunctionDef) -> bool {
    def.volatility == Volatility::Volatile
}
