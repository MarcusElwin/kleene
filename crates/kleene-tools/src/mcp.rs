//! Model Context Protocol servers as tools.
//!
//! An MCP server is a child process speaking JSON-RPC 2.0 over stdio, one
//! JSON object per line. [`McpClient::connect`] starts one, runs the
//! `initialize` handshake and lists its tools; each becomes an [`McpTool`]
//! in the catalog under `<server>_<tool>`. A tool whose annotations say
//! `readOnlyHint: true` is `STABLE` (a table function the model uses in
//! `FROM`); every other tool is `VOLATILE` and runs only through `CALL`,
//! because the planner trusts that label and an unmarked side effect must
//! never be reordered or deduplicated.
//!
//! Servers are configured in `mcp.json` ([`load_config`]), the format every
//! MCP client reads:
//!
//! ```json
//! { "mcpServers": { "fs": { "command": "npx", "args": ["-y", "server-filesystem", "."], "env": {} } } }
//! ```

use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

/// Protocol version this client announces.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// How long the handshake and the tool listing may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one tool call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// One server in `mcp.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Name, the prefix of its tools in the catalog.
    #[serde(skip)]
    pub name: String,
    /// Program to run.
    pub command: String,
    /// Its arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: BTreeMap<String, McpServerConfig>,
}

/// The user and project `mcp.json` paths for a workspace and a config
/// directory, in loading order (the project file wins on a name clash).
pub fn config_paths(workspace: &Path, config_dir: &Path) -> [PathBuf; 2] {
    [
        config_dir.join("mcp.json"),
        workspace.join(".kleene").join("mcp.json"),
    ]
}

/// The servers listed in one `mcp.json`; a missing file is no servers.
pub fn load_config(path: &Path) -> Result<Vec<McpServerConfig>, ToolError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let file: ConfigFile = serde_json::from_str(&text)
        .map_err(|e| ToolError::Other(format!("{}: {e}", path.display())))?;
    Ok(file
        .mcp_servers
        .into_iter()
        .map(|(name, mut c)| {
            c.name = name;
            c
        })
        .collect())
}

/// Every configured server for a workspace: the user file, then the
/// project file replacing same-named entries. Sorted by name. An unreadable
/// file is skipped with its error returned beside the list.
pub fn load_all(workspace: &Path, config_dir: &Path) -> (Vec<McpServerConfig>, Vec<String>) {
    let mut by_name: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    let mut errors = vec![];
    for path in config_paths(workspace, config_dir) {
        match load_config(&path) {
            Ok(list) => {
                for c in list {
                    by_name.insert(c.name.clone(), c);
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    (by_name.into_values().collect(), errors)
}

/// Add (or replace) a server in an `mcp.json`, creating the file.
pub fn add_server(path: &Path, server: &McpServerConfig) -> Result<(), ToolError> {
    let mut file = read_file(path)?;
    file.mcp_servers.insert(server.name.clone(), server.clone());
    write_file(path, &file)
}

/// Remove a server from an `mcp.json`; returns whether it was there.
pub fn remove_server(path: &Path, name: &str) -> Result<bool, ToolError> {
    let mut file = read_file(path)?;
    let found = file.mcp_servers.remove(name).is_some();
    if found {
        write_file(path, &file)?;
    }
    Ok(found)
}

fn read_file(path: &Path) -> Result<ConfigFile, ToolError> {
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t)
            .map_err(|e| ToolError::Other(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(e) => Err(e.into()),
    }
}

fn write_file(path: &Path, file: &ConfigFile) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| ToolError::Other(e.to_string()))?;
    std::fs::write(path, text + "\n")?;
    Ok(())
}

/// A tool's positional parameters: name and SQL type, in order.
pub type Params = Vec<(String, DataType)>;

/// A tool as the server described it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpToolInfo {
    /// Name on the server.
    pub name: String,
    /// Description, if any.
    #[serde(default)]
    pub description: String,
    /// JSON schema of the arguments.
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
    /// Hints such as `readOnlyHint`.
    #[serde(default)]
    pub annotations: serde_json::Value,
}

impl McpToolInfo {
    /// Whether the server marks the tool read-only.
    pub fn read_only(&self) -> bool {
        self.annotations
            .get("readOnlyHint")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// The positional parameters: required ones in the schema's `required`
    /// order, then the optional ones by name, each with the SQL type its
    /// JSON type maps to.
    pub fn params(&self) -> (Params, Params) {
        let props = self
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();
        let required: Vec<String> = self
            .input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|r| {
                r.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .filter(|n| props.contains_key(n))
                    .collect()
            })
            .unwrap_or_default();
        let ty = |name: &str| -> DataType {
            match props
                .get(name)
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
            {
                Some("integer") => DataType::Int,
                Some("number") => DataType::Float,
                Some("boolean") => DataType::Bool,
                _ => DataType::Text,
            }
        };
        let req: Vec<(String, DataType)> = required.iter().map(|n| (n.clone(), ty(n))).collect();
        let mut opt: Vec<(String, DataType)> = props
            .keys()
            .filter(|k| !required.contains(k))
            .map(|k| (k.clone(), ty(k)))
            .collect();
        opt.sort_by(|a, b| a.0.cmp(&b.0));
        (req, opt)
    }
}

/// What one connection attempt produced, for `/mcp` and `kleene mcp list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStatus {
    /// Server name.
    pub server: String,
    /// The command line.
    pub command: String,
    /// `connected`, or `error: ...`.
    pub state: String,
    /// Catalog names of its tools.
    pub tools: Vec<String>,
}

/// A running server.
pub struct McpClient {
    name: String,
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    next_id: AtomicU64,
    _child: Child,
    _reader: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("name", &self.name)
            .finish()
    }
}

impl McpClient {
    /// Start the server, complete the handshake and list its tools.
    pub async fn connect(
        cfg: &McpServerConfig,
    ) -> Result<(Arc<Self>, Vec<McpToolInfo>), ToolError> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args).env_clear();
        for key in ["PATH", "HOME", "LANG", "TMPDIR", "USER"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd.envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Other(format!("cannot start {}: {e}", cfg.command)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::Other("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::Other("no stdout".into()))?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let reader = {
            let pending = pending.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let Some(id) = msg.get("id").and_then(|i| i.as_u64()) else {
                        continue; // a notification or a request from the server
                    };
                    let tx = pending.lock().ok().and_then(|mut p| p.remove(&id));
                    if let Some(tx) = tx {
                        let _ = tx.send(msg);
                    }
                }
            })
        };
        let client = Arc::new(Self {
            name: cfg.name.clone(),
            stdin: tokio::sync::Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            _child: child,
            _reader: reader,
        });
        let init = client
            .request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "kleene", "version": env!("CARGO_PKG_VERSION")}
                }),
                CONNECT_TIMEOUT,
            )
            .await?;
        let _ = init;
        client
            .notify("notifications/initialized", serde_json::json!({}))
            .await?;
        let mut tools = vec![];
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => serde_json::json!({"cursor": c}),
                None => serde_json::json!({}),
            };
            let listed = client
                .request("tools/list", params, CONNECT_TIMEOUT)
                .await?;
            if let Some(arr) = listed.get("tools").and_then(|t| t.as_array()) {
                for t in arr {
                    if let Ok(info) = serde_json::from_value::<McpToolInfo>(t.clone()) {
                        tools.push(info);
                    }
                }
            }
            cursor = listed
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok((client, tools))
    }

    /// The server's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    async fn send(&self, msg: &serde_json::Value) -> Result<(), ToolError> {
        let mut line = serde_json::to_string(msg).map_err(|e| ToolError::Other(e.to_string()))?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| ToolError::Other(format!("{}: write failed: {e}", self.name)))?;
        stdin
            .flush()
            .await
            .map_err(|e| ToolError::Other(format!("{}: write failed: {e}", self.name)))?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), ToolError> {
        self.send(&serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// One JSON-RPC request; the `result`, or the error's message.
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, tx);
        }
        self.send(
            &serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .await?;
        let reply = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => {
                return Err(ToolError::Other(format!(
                    "{}: server closed the connection",
                    self.name
                )))
            }
            Err(_) => {
                if let Ok(mut p) = self.pending.lock() {
                    p.remove(&id);
                }
                return Err(ToolError::Timeout(timeout.as_millis() as u64));
            }
        };
        if let Some(err) = reply.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(ToolError::Other(format!("{}: {method}: {msg}", self.name)));
        }
        Ok(reply
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

/// Catalog name for a server's tool: `<server>_<tool>`, lower-cased, with
/// every character that is not a letter, digit or underscore replaced by
/// an underscore.
pub fn catalog_name(server: &str, tool: &str) -> String {
    format!("{server}_{tool}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// One MCP tool in the catalog.
#[derive(Debug)]
pub struct McpTool {
    client: Arc<McpClient>,
    info: McpToolInfo,
    name: String,
    description: String,
    required: Params,
    optional: Params,
    schema: Arc<Schema>,
}

impl McpTool {
    /// Wrap a listed tool of a connected server.
    pub fn new(client: Arc<McpClient>, info: McpToolInfo) -> Self {
        let (required, optional) = info.params();
        let name = catalog_name(client.name(), &info.name);
        let sig: Vec<String> = required
            .iter()
            .map(|(n, t)| format!("{n} {t}"))
            .chain(optional.iter().map(|(n, t)| format!("[{n} {t}]")))
            .collect();
        let description = format!(
            "{}({}): {}",
            info.name,
            sig.join(", "),
            info.description.lines().next().unwrap_or_default().trim()
        );
        Self {
            client,
            info,
            name,
            description,
            required,
            optional,
            schema: Arc::new(Schema::new(vec![Field::not_null("text", DataType::Text)])),
        }
    }

    /// The server's own name for the tool.
    pub fn server_tool(&self) -> &str {
        &self.info.name
    }

    fn arguments(&self, args: &[Value]) -> Result<serde_json::Value, ToolError> {
        let params: Vec<&(String, DataType)> =
            self.required.iter().chain(self.optional.iter()).collect();
        let mut out = serde_json::Map::new();
        for (i, v) in args.iter().enumerate() {
            let Some((name, ty)) = params.get(i).map(|p| (&p.0, &p.1)) else {
                break;
            };
            let json = match (v, ty) {
                (Value::Null, _) => continue,
                (Value::Int(n), _) => serde_json::json!(n),
                (Value::Float(f), _) => serde_json::json!(f),
                (Value::Bool(b), _) => serde_json::json!(b),
                (Value::Text(s), DataType::Int) => {
                    serde_json::json!(s.trim().parse::<i64>().map_err(|_| ToolError::Args(
                        format!("{name} must be an integer, got {s:?}")
                    ))?)
                }
                (Value::Text(s), DataType::Float) => serde_json::json!(s
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| ToolError::Args(format!("{name} must be a number, got {s:?}")))?),
                (Value::Text(s), DataType::Bool) => serde_json::json!(matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "true" | "t" | "yes" | "1"
                )),
                (Value::Text(s), _) => {
                    let declared = self
                        .info
                        .input_schema
                        .get("properties")
                        .and_then(|p| p.get(name))
                        .and_then(|p| p.get("type"))
                        .and_then(|t| t.as_str());
                    match declared {
                        Some("object") | Some("array") => serde_json::from_str(s).map_err(|e| {
                            ToolError::Args(format!("{name} must be JSON ({declared:?}): {e}"))
                        })?,
                        _ => serde_json::json!(s),
                    }
                }
                (other, _) => serde_json::json!(other.render()),
            };
            out.insert(name.clone(), json);
        }
        Ok(serde_json::Value::Object(out))
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> Signature {
        Signature::new(
            &self.required.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            &self.optional.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
        )
    }

    fn schema(&self) -> Arc<Schema> {
        self.schema.clone()
    }

    fn volatility(&self) -> Volatility {
        if self.info.read_only() {
            Volatility::Stable
        } else {
            Volatility::Volatile
        }
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn call(&self, args: &[Value], _ctx: &ToolContext) -> Result<Batch, ToolError> {
        if args.len() < self.required.len() {
            return Err(ToolError::Args(format!(
                "{} needs {} argument(s) ({}), got {}",
                self.name,
                self.required.len(),
                self.required
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                args.len()
            )));
        }
        let arguments = self.arguments(args)?;
        let result = self
            .client
            .request(
                "tools/call",
                serde_json::json!({"name": self.info.name, "arguments": arguments}),
                CALL_TIMEOUT,
            )
            .await?;
        let mut rows: Vec<Vec<Value>> = vec![];
        if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
            for item in items {
                let text = match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    Some(other) => format!("[{other} content omitted]"),
                    None => item.to_string(),
                };
                rows.push(vec![Value::Text(text)]);
            }
        }
        if rows.is_empty() {
            if let Some(s) = result.get("structuredContent") {
                rows.push(vec![Value::Text(s.to_string())]);
            }
        }
        if result
            .get("isError")
            .and_then(|e| e.as_bool())
            .unwrap_or(false)
        {
            let text: Vec<String> = rows.iter().map(|r| r[0].render()).collect();
            return Err(ToolError::Other(format!(
                "{} failed: {}",
                self.name,
                text.join("\n")
            )));
        }
        Ok(Batch {
            schema: self.schema.clone(),
            rows,
        })
    }
}

/// Connect every configured server and wrap its tools. A server that fails
/// is reported in its status and contributes no tools.
pub async fn connect_all(configs: &[McpServerConfig]) -> (Vec<Arc<dyn Tool>>, Vec<McpStatus>) {
    let mut tools: Vec<Arc<dyn Tool>> = vec![];
    let mut statuses = vec![];
    for cfg in configs {
        let command = std::iter::once(cfg.command.clone())
            .chain(cfg.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        match McpClient::connect(cfg).await {
            Ok((client, infos)) => {
                let mut names = vec![];
                for info in infos {
                    let t = McpTool::new(client.clone(), info);
                    names.push(t.name().to_string());
                    tools.push(Arc::new(t));
                }
                statuses.push(McpStatus {
                    server: cfg.name.clone(),
                    command,
                    state: "connected".into(),
                    tools: names,
                });
            }
            Err(e) => statuses.push(McpStatus {
                server: cfg.name.clone(),
                command,
                state: format!("error: {e}"),
                tools: vec![],
            }),
        }
    }
    (tools, statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_project_wins() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("cfg").join("mcp.json");
        let ws = dir.path().join("ws");
        add_server(
            &user,
            &McpServerConfig {
                name: "fs".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "server-filesystem".into()],
                env: BTreeMap::new(),
            },
        )
        .unwrap();
        add_server(
            &ws.join(".kleene").join("mcp.json"),
            &McpServerConfig {
                name: "fs".into(),
                command: "my-fs".into(),
                args: vec![],
                env: [("K".to_string(), "v".to_string())].into_iter().collect(),
            },
        )
        .unwrap();
        let (all, errors) = load_all(&ws, dir.path().join("cfg").as_path());
        assert!(errors.is_empty());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].command, "my-fs");
        assert_eq!(all[0].env["K"], "v");
        assert!(remove_server(&user, "fs").unwrap());
        assert!(!remove_server(&user, "fs").unwrap());
        let text = std::fs::read_to_string(&user).unwrap();
        assert!(text.contains("\"mcpServers\": {}"), "{text}");
    }

    #[test]
    fn params_follow_required_order_then_optional_by_name() {
        let info: McpToolInfo = serde_json::from_value(serde_json::json!({
            "name": "read_file",
            "description": "Read a file.\nMore.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}, "limit": {"type": "integer"},
                "offset": {"type": "number"}, "raw": {"type": "boolean"}, "opts": {"type": "object"}},
                "required": ["path", "limit"]},
            "annotations": {"readOnlyHint": true}
        }))
        .unwrap();
        let (req, opt) = info.params();
        assert_eq!(
            req,
            vec![
                ("path".into(), DataType::Text),
                ("limit".into(), DataType::Int)
            ]
        );
        assert_eq!(
            opt,
            vec![
                ("offset".into(), DataType::Float),
                ("opts".into(), DataType::Text),
                ("raw".into(), DataType::Bool)
            ]
        );
        assert!(info.read_only());
        assert_eq!(
            catalog_name("Files Server", "read/file"),
            "files_server_read_file"
        );
    }

    /// A fake MCP server in `sh`: answers `initialize`, `tools/list` and
    /// `tools/call` for one `echo` tool with canned JSON lines.
    fn fake_server(dir: &Path) -> McpServerConfig {
        let script = dir.join("server.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"0"}}}' ;;
    *'"method":"tools/list"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo text back","inputSchema":{"type":"object","properties":{"text":{"type":"string"},"times":{"type":"integer"}},"required":["text"]},"annotations":{"readOnlyHint":true}},{"name":"boom","description":"Always fails","inputSchema":{"type":"object","properties":{}}}]}}' ;;
    *'"name":"boom"'*) echo '{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"it broke"}],"isError":true}}' ;;
    *'"method":"tools/call"'*) echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"hello"},{"type":"image","data":"..."}]}}' ;;
  esac
done
"#,
        )
        .unwrap();
        McpServerConfig {
            name: "fake".into(),
            command: "sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn connects_lists_and_calls_a_stdio_server() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = fake_server(dir.path());
        let (tools, status) = connect_all(&[cfg]).await;
        assert_eq!(status[0].state, "connected", "{status:?}");
        assert_eq!(status[0].tools, vec!["fake_echo", "fake_boom"]);
        let echo = tools.iter().find(|t| t.name() == "fake_echo").unwrap();
        assert_eq!(echo.volatility(), Volatility::Stable);
        assert_eq!(echo.signature().required, vec![DataType::Text]);
        assert_eq!(echo.signature().optional, vec![DataType::Int]);
        assert_eq!(
            echo.description(),
            "echo(text TEXT, [times BIGINT]): Echo text back"
        );
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let b = echo.call(&[Value::from("hi")], &ctx).await.unwrap();
        let rows: Vec<String> = b.rows.iter().map(|r| r[0].render()).collect();
        assert_eq!(rows, vec!["hello", "[image content omitted]"]);
        let boom = tools.iter().find(|t| t.name() == "fake_boom").unwrap();
        assert_eq!(boom.volatility(), Volatility::Volatile);
        let err = boom.call(&[], &ctx).await.unwrap_err();
        assert_eq!(err.to_string(), "fake_boom failed: it broke");
        let err = echo.call(&[], &ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("needs 1 argument(s) (text)"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_is_reported_not_fatal() {
        let (tools, status) = connect_all(&[McpServerConfig {
            name: "missing".into(),
            command: "/nonexistent/mcp-server".into(),
            args: vec![],
            env: BTreeMap::new(),
        }])
        .await;
        assert!(tools.is_empty());
        assert!(
            status[0].state.starts_with("error: cannot start"),
            "{status:?}"
        );
    }
}
