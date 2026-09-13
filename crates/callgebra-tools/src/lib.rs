//! The tool surface, as relations and functions.
//!
//! Everything the model does is SQL, so every tool is either a virtual
//! relation (`files`, `INBOX`), a table function (`grep`, `chunks`) or a
//! `CALL` statement (`shell`, `write_file`). Each carries a [`Volatility`]
//! the planner respects. Implementations arrive in M2; the sandbox in M3.

#![forbid(unsafe_code)]

use callgebra_core::{Batch, Schema, Value, Volatility};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Tool failures. Rendered back to the model as a result row.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Path outside the workspace or not on the role's allow-list.
    #[error("access denied: {0}")]
    Denied(String),
    /// Filesystem or process error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Bad arguments.
    #[error("invalid arguments: {0}")]
    Args(String),
    /// The command exceeded its time limit.
    #[error("timed out after {0} ms")]
    Timeout(u64),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// What a tool runs against: the workspace root, permissions, limits.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Root of the session workspace; every path is resolved under it.
    pub workspace: PathBuf,
    /// Paths (relative to the workspace) the role may write.
    pub writable: Vec<PathBuf>,
    /// Default timeout for `shell`, in milliseconds.
    pub shell_timeout_ms: u64,
    /// Allow-listed environment variable names for `env()`.
    pub env_allowlist: Vec<String>,
}

/// A tool exposed to SQL.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Catalog name (`files`, `grep`, `shell`, ...).
    fn name(&self) -> &str;
    /// Output columns.
    fn schema(&self) -> Arc<Schema>;
    /// How stable the result is; drives planner fences and memoisation.
    fn volatility(&self) -> Volatility;
    /// One-line description for the prompt catalog.
    fn description(&self) -> &str;
    /// Evaluate on one argument tuple.
    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError>;
}

/// Tools by name.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }
    /// Register a tool under its name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_ascii_lowercase(), tool);
    }
    /// Look a tool up.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(&name.to_ascii_lowercase())
    }
    /// All tools, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }
}
