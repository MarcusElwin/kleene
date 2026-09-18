//! The tool surface, as relations and functions.
//!
//! Everything the model does is SQL, so every tool is either a table function
//! the model uses in `FROM` (`files`, `grep`, `chunks`) or a `CALL` statement
//! (`shell`, `write_file`). Each carries a [`Volatility`] the planner
//! respects: `STABLE` tools may be reordered and deduplicated within a
//! statement, `VOLATILE` tools are fences evaluated once per input row and
//! only reachable through `CALL` (see [`side_effect_tools`]).
//!
//! The standard surface, as the model sees it in the catalog:
//!
//! | tool | signature | columns | volatility |
//! |---|---|---|---|
//! | `files` | `(glob TEXT)` | `path, size, mtime, kind` | STABLE |
//! | `lines` | `(path TEXT)` | `lineno, text` | STABLE |
//! | `grep` | `(pattern TEXT [, glob TEXT])` | `path, lineno, text` | STABLE |
//! | `read` | `(path TEXT)` | `text` | STABLE |
//! | `chunks` | `(text TEXT, size BIGINT [, overlap BIGINT])` | `ordinal, text, tokens` | IMMUTABLE |
//! | `env` | `(name TEXT)` | `value` | STABLE |
//! | `git_log` | `([n BIGINT])` | `sha, author, date, message` | STABLE |
//! | `git_diff` | `([ref TEXT])` | `path, diff` | STABLE |
//! | `git_blame` | `(path TEXT)` | `lineno, sha, author, text` | STABLE |
//! | `shell` | `(cmd TEXT [, cwd TEXT, timeout_ms BIGINT])` | `stdout, stderr, exit_code, duration_ms` | VOLATILE |
//! | `write_file` | `(path TEXT, text TEXT)` | `path, bytes` | VOLATILE |
//! | `append_file` | `(path TEXT, text TEXT)` | `path, bytes` | VOLATILE |
//! | `patch` | `(path TEXT, old TEXT, new TEXT)` | `path, replaced` | VOLATILE |
//! | `mkdir` | `(path TEXT)` | `path` | VOLATILE |
//! | `remove` | `(path TEXT)` | `path` | VOLATILE |
//! | `web_fetch` | `(url TEXT)` | `url, status, text, tokens` | VOLATILE |
//! | `web_search` | `(q TEXT [, n BIGINT])` | `rank, title, url, snippet` | VOLATILE |
//!
//! Every path argument is resolved under the session workspace by
//! [`paths`]; nothing outside it is readable, and writes are further limited
//! to the [`ToolContext::writable`] allow-list. `shell` runs as a plain
//! subprocess in M2; the sandbox arrives in M3.

#![forbid(unsafe_code)]

pub mod paths;
pub mod tools;

mod args;

use kleene_core::{
    Batch, CallKind, DataType, FunctionDef, FunctionReturn, Schema, Value, Volatility,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
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

/// Files the model has read, with the modification time seen at that read.
///
/// `patch` consults it: a file may only be patched if it was read and has not
/// changed since. Keys are canonical absolute paths.
pub type ReadRegistry = Arc<Mutex<HashMap<PathBuf, SystemTime>>>;

/// What a tool runs against: the workspace root, permissions, limits.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Root of the session workspace; every path is resolved under it.
    pub workspace: PathBuf,
    /// Paths the role may write: absolute, or relative to the workspace.
    pub writable: Vec<PathBuf>,
    /// Default timeout for `shell`, in milliseconds.
    pub shell_timeout_ms: u64,
    /// Allow-listed environment variable names for `env()`.
    pub env_allowlist: Vec<String>,
    /// Host names `web_fetch` may contact (exact, case-insensitive match).
    pub network_allowlist: Vec<String>,
    /// Modification time of each file at its last `read`/`lines`; shared by
    /// every tool of a session so `patch` can refuse stale edits.
    pub reads: ReadRegistry,
}

impl ToolContext {
    /// A context rooted at `workspace` with the defaults: the whole workspace
    /// writable, a 30 s shell timeout, and empty environment and network
    /// allow-lists.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            writable: vec![workspace.clone()],
            workspace,
            shell_timeout_ms: 30_000,
            env_allowlist: Vec::new(),
            network_allowlist: Vec::new(),
            reads: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record that `path` (canonical, absolute) was read while it had `mtime`.
    pub fn record_read(&self, path: PathBuf, mtime: SystemTime) {
        if let Ok(mut reads) = self.reads.lock() {
            reads.insert(path, mtime);
        }
    }

    /// The modification time recorded at the last read of `path`, if any.
    pub fn last_read(&self, path: &Path) -> Option<SystemTime> {
        self.reads.lock().ok().and_then(|r| r.get(path).copied())
    }
}

/// A tool's argument list: the leading required arguments and the optional
/// trailing ones. The catalog exposes the required ones as `args` and marks
/// the function `variadic` when any optional argument exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// Arguments that must be given, in order.
    pub required: Vec<DataType>,
    /// Arguments that may follow, in order.
    pub optional: Vec<DataType>,
}

impl Signature {
    /// A signature with the given required and optional argument types.
    pub fn new(required: &[DataType], optional: &[DataType]) -> Self {
        Self {
            required: required.to_vec(),
            optional: optional.to_vec(),
        }
    }

    /// Smallest accepted argument count.
    pub fn min_args(&self) -> usize {
        self.required.len()
    }

    /// Largest accepted argument count.
    pub fn max_args(&self) -> usize {
        self.required.len() + self.optional.len()
    }
}

/// A tool exposed to SQL.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Catalog name (`files`, `grep`, `shell`, ...).
    fn name(&self) -> &str;
    /// Argument types.
    fn signature(&self) -> Signature;
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
    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }
    /// `true` if nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// The standard tool surface: every tool listed in the crate documentation.
pub fn standard_tools() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    let all: Vec<Arc<dyn Tool>> = vec![
        Arc::new(tools::files::Files),
        Arc::new(tools::lines::Lines),
        Arc::new(tools::grep::Grep),
        Arc::new(tools::read::Read),
        Arc::new(tools::chunks::Chunks),
        Arc::new(tools::env::Env),
        Arc::new(tools::git_log::GitLog),
        Arc::new(tools::git_diff::GitDiff),
        Arc::new(tools::git_blame::GitBlame),
        Arc::new(tools::shell::Shell),
        Arc::new(tools::write_file::WriteFile),
        Arc::new(tools::append_file::AppendFile),
        Arc::new(tools::patch::Patch),
        Arc::new(tools::mkdir::Mkdir),
        Arc::new(tools::remove::Remove),
        Arc::new(tools::web_fetch::WebFetch),
        Arc::new(tools::web_search::WebSearch),
    ];
    for t in all {
        reg.register(t);
    }
    reg
}

/// Names of the `VOLATILE` tools: the side-effect surface the planner only
/// admits inside `CALL`, never in an expression or a `FROM` clause.
pub fn side_effect_tools() -> &'static [&'static str] {
    &[
        "append_file",
        "mkdir",
        "patch",
        "remove",
        "shell",
        "web_fetch",
        "web_search",
        "write_file",
    ]
}

/// One table-valued [`FunctionDef`] per registered tool, so the SQL frontend
/// resolves `SELECT * FROM grep('foo')` and `CROSS JOIN LATERAL lines(path)`.
pub fn catalog_entries(reg: &ToolRegistry) -> Vec<FunctionDef> {
    reg.iter()
        .map(|t| {
            let sig = t.signature();
            FunctionDef {
                name: t.name().to_string(),
                args: sig.required,
                variadic: !sig.optional.is_empty(),
                returns: FunctionReturn::Table {
                    schema: (*t.schema()).clone(),
                },
                call_kind: CallKind::Tool {
                    tool: t.name().to_string(),
                },
                volatility: t.volatility(),
                description: t.description().to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_one_entry_per_tool_with_its_volatility() {
        let reg = standard_tools();
        let defs = catalog_entries(&reg);
        assert_eq!(defs.len(), reg.len());
        assert_eq!(reg.len(), 17);
        for def in &defs {
            let tool = reg.get(&def.name).expect("entry names a tool");
            assert_eq!(def.volatility, tool.volatility(), "{}", def.name);
            assert_eq!(
                def.call_kind,
                CallKind::Tool {
                    tool: def.name.clone()
                }
            );
            assert!(matches!(def.returns, FunctionReturn::Table { .. }));
            assert!(!def.description.is_empty());
        }
        let by_name = |n: &str| defs.iter().find(|d| d.name == n).unwrap();
        assert_eq!(by_name("chunks").volatility, Volatility::Immutable);
        assert_eq!(by_name("grep").args, vec![DataType::Text]);
        assert!(by_name("grep").variadic);
        assert!(!by_name("lines").variadic);
        assert_eq!(by_name("shell").volatility, Volatility::Volatile);
        assert_eq!(by_name("git_log").volatility, Volatility::Stable);
        assert!(by_name("git_log").args.is_empty());
    }

    #[test]
    fn side_effect_list_matches_volatile_tools() {
        let reg = standard_tools();
        let mut volatile: Vec<&str> = reg
            .iter()
            .filter(|t| t.volatility() == Volatility::Volatile)
            .map(|t| t.name())
            .collect();
        volatile.sort_unstable();
        assert_eq!(volatile, side_effect_tools());
    }

    #[test]
    fn context_defaults() {
        let ctx = ToolContext::new(PathBuf::from("/ws"));
        assert_eq!(ctx.writable, vec![PathBuf::from("/ws")]);
        assert_eq!(ctx.shell_timeout_ms, 30_000);
        assert!(ctx.env_allowlist.is_empty());
        assert!(ctx.network_allowlist.is_empty());
        assert!(ctx.last_read(Path::new("/ws/a")).is_none());
    }
}
