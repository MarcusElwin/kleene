//! The plain-agent baseline: a tool-calling loop on the same provider,
//! tools and budgets as the harness, without SQL. On a provider with native
//! tool calling the tools are sent as tool definitions and the model's
//! `tool_use` blocks are executed, exactly as a vendor agent loop does; the
//! answer comes back through a `final` tool. On a provider without it (the
//! scripted test provider, some gateways) each turn the model returns one
//! JSON action, `{"tool": name, "args": [...]}` or
//! `{"final": [[cell, ...], ...], "columns": [...]}`, and tool output is fed
//! back as text. This is the control the RLM-paper framing needs: same
//! model, same client, same spend, only the abstraction differs.

use kleene_core::{Batch, Budget, BudgetUsage, DataType, Field, Schema, Value};
use kleene_llm::{
    CompletionRequest, ContentBlock, Message, Provider, ProviderOptions, Role, ToolDef,
};
use kleene_tools::{ToolContext, ToolRegistry};
use std::path::PathBuf;
use std::sync::Arc;

/// Configuration of a plain-agent run.
#[derive(Clone)]
pub struct PlainConfig {
    /// Model provider.
    pub provider: Arc<dyn Provider>,
    /// Alias to call.
    pub alias: String,
    /// Workspace for tools.
    pub workspace: PathBuf,
    /// Turn cap.
    pub max_turns: u32,
    /// Spending cap.
    pub budget: Budget,
    /// Tokens per reply.
    pub max_tokens: u32,
    /// A shell command that must exit 0 before `final` is accepted; a
    /// failing check is fed back and the loop continues.
    pub check: Option<String>,
}

/// What a plain-agent run produced.
#[derive(Debug, Clone)]
pub struct PlainReport {
    /// The final relation, if the agent finished.
    pub answer: Option<Batch>,
    /// Why it stopped: `final`, `turns_exhausted`, `budget_exhausted`, `error`.
    pub outcome: String,
    /// Turns taken.
    pub turns: u32,
    /// Spending.
    pub usage: BudgetUsage,
}

/// The name of the tool that ends a native run.
pub const FINAL_TOOL: &str = "final";

fn signature_text(tool: &dyn kleene_tools::Tool) -> String {
    let sig = tool.signature();
    let mut parts: Vec<String> = sig.required.iter().map(|t| t.to_string()).collect();
    parts.extend(sig.optional.iter().map(|t| format!("[{t}]")));
    let cols: Vec<String> = tool
        .schema()
        .fields
        .iter()
        .map(|f| f.name.clone())
        .collect();
    format!(
        "{}({}) -> ({})",
        tool.name(),
        parts.join(", "),
        cols.join(", ")
    )
}

fn system_prompt(tools: &ToolRegistry) -> String {
    let mut out = String::from(
        "You are an agent that solves a task by calling tools, one per turn. \
Reply with exactly one JSON object and nothing else. To call a tool: {\"tool\": \"name\", \"args\": [arg1, arg2]}. \
To finish: {\"final\": [[cell, ...], ...], \"columns\": [\"name\", ...]} where each inner list is one row of your answer relation. \
Tools available:\n",
    );
    for t in tools.iter() {
        out.push_str(&format!(
            "- {}({} required arg(s)): {}\n",
            t.name(),
            t.signature().required.len(),
            t.description()
        ));
    }
    out
}

/// The tool definitions a native run sends: every registry tool with its
/// positional arguments as one `args` array, plus `final`.
pub fn tool_defs(tools: &ToolRegistry) -> Vec<ToolDef> {
    let mut defs: Vec<ToolDef> = tools
        .iter()
        .map(|t| ToolDef {
            name: t.name().to_string(),
            description: format!(
                "{}. Signature: {}",
                t.description(),
                signature_text(t.as_ref())
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {},
                        "description": "positional arguments, in the signature's order"
                    }
                },
                "required": ["args"]
            }),
        })
        .collect();
    defs.push(ToolDef {
        name: FINAL_TOOL.into(),
        description:
            "Finish the task with your answer as a small relation: column names and rows of cells."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "columns": {"type": "array", "items": {"type": "string"}},
                "rows": {"type": "array", "items": {"type": "array", "items": {}}}
            },
            "required": ["columns", "rows"]
        }),
    });
    defs
}

/// Run one task with the plain agent.
pub async fn run(cfg: &PlainConfig, task: &str, context: Option<&str>) -> PlainReport {
    let tools = kleene_tools::standard_tools();
    let ctx = ToolContext::new(cfg.workspace.clone());
    let task_text = match context {
        Some(c) => format!("# Task\n{task}\n\n# Context\n{c}"),
        None => format!("# Task\n{task}"),
    };
    if cfg.provider.capabilities().tools {
        run_native(cfg, &tools, &ctx, task_text).await
    } else {
        run_json(cfg, &tools, &ctx, task_text).await
    }
}

/// `Some(text)` when the finish check fails.
async fn check_fails(cfg: &PlainConfig, tools: &ToolRegistry, ctx: &ToolContext) -> Option<String> {
    let cmd = cfg.check.as_deref()?;
    let shell = tools.get("shell")?;
    let args = [
        Value::Text(cmd.to_string()),
        Value::Null,
        Value::Int(300_000),
    ];
    let (code, output) = match shell.call(&args, ctx).await {
        Ok(b) => {
            let row = b.rows.first()?;
            (
                row.get(2).and_then(|v| v.as_int()).unwrap_or(-1),
                format!(
                    "{}\n{}",
                    row.first().map(|v| v.render()).unwrap_or_default(),
                    row.get(1).map(|v| v.render()).unwrap_or_default()
                ),
            )
        }
        Err(e) => (-1, e.to_string()),
    };
    if code == 0 {
        return None;
    }
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(40)..].join("\n");
    Some(format!(
        "final refused: the check `{cmd}` exited {code}. Its output ends:\n{tail}\nThe task is not done while this fails: fix the code, run the check, then finish again."
    ))
}

fn relation(columns: &[serde_json::Value], rows: &[serde_json::Value]) -> Batch {
    let columns: Vec<String> = columns
        .iter()
        .map(|v| v.as_str().unwrap_or("col").to_string())
        .collect();
    let width = rows
        .iter()
        .filter_map(|r| r.as_array())
        .map(|r| r.len())
        .max()
        .unwrap_or(columns.len().max(1))
        .max(columns.len());
    let fields: Vec<Field> = (0..width)
        .map(|i| {
            Field::new(
                columns
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("col{}", i + 1)),
                DataType::Text,
            )
        })
        .collect();
    let rows: Vec<Vec<Value>> = rows
        .iter()
        .map(|r| {
            let cells = r.as_array().cloned().unwrap_or_else(|| vec![r.clone()]);
            (0..width)
                .map(|i| match cells.get(i) {
                    Some(serde_json::Value::String(s)) => Value::Text(s.clone()),
                    Some(serde_json::Value::Null) | None => Value::Null,
                    Some(other) => Value::Text(other.to_string()),
                })
                .collect()
        })
        .collect();
    Batch {
        schema: Arc::new(Schema::new(fields)),
        rows,
    }
}

fn json_args(args: Option<&serde_json::Value>) -> Vec<Value> {
    args.and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => Value::Text(s.clone()),
                    serde_json::Value::Number(n) => n
                        .as_i64()
                        .map(Value::Int)
                        .unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
                    serde_json::Value::Bool(b) => Value::Bool(*b),
                    serde_json::Value::Null => Value::Null,
                    other => Value::Text(other.to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn call_tool(
    tools: &ToolRegistry,
    ctx: &ToolContext,
    name: &str,
    args: &[Value],
) -> (String, bool) {
    match tools.get(name) {
        None => (
            format!(
                "unknown tool {name:?}; tools: {}",
                tools
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            true,
        ),
        Some(tool) => match tool.call(args, ctx).await {
            Ok(batch) => (
                format!("{}\n{} row(s)", batch.render_table(40), batch.rows.len()),
                false,
            ),
            Err(e) => (format!("error: {e}"), true),
        },
    }
}

struct Spend {
    usage: BudgetUsage,
    started: std::time::Instant,
}

impl Spend {
    fn report(mut self, answer: Option<Batch>, outcome: &str, turns: u32) -> PlainReport {
        self.usage.wall = self.started.elapsed();
        PlainReport {
            answer,
            outcome: outcome.into(),
            turns,
            usage: self.usage,
        }
    }

    /// Whether one more call fits the budget.
    fn affords(&self, budget: &Budget) -> bool {
        let mut next = self.usage;
        next.calls += 1;
        budget.check(&next).is_ok()
    }

    fn charge(&mut self, resp: &kleene_llm::CompletionResponse) {
        self.usage.calls += 1;
        self.usage.tokens += resp.usage.total_tokens();
        self.usage.dollars += resp.usage.cost_usd.unwrap_or(0.0);
    }
}

async fn run_native(
    cfg: &PlainConfig,
    tools: &ToolRegistry,
    ctx: &ToolContext,
    task_text: String,
) -> PlainReport {
    let defs = tool_defs(tools);
    let system = format!(
        "You are an agent that solves a task by calling tools. Explore with the read-only tools, change things with the others, and run commands with shell. \
When the task is done, call {FINAL_TOOL} with your answer relation. Keep going until it is done; a failing command or test is the next thing to fix.{}",
        match &cfg.check {
            Some(c) => format!(" {FINAL_TOOL} is accepted only when `{c}` exits 0; run it yourself first."),
            None => String::new(),
        }
    );
    let mut messages = vec![Message::user(task_text)];
    let mut spend = Spend {
        usage: BudgetUsage::default(),
        started: std::time::Instant::now(),
    };
    for turn in 1..=cfg.max_turns {
        if !spend.affords(&cfg.budget) {
            return spend.report(None, "budget_exhausted", turn - 1);
        }
        let req = CompletionRequest {
            alias: kleene_core::ModelAlias(cfg.alias.clone()),
            model: cfg.alias.clone(),
            system: system.clone(),
            messages: messages.clone(),
            tools: defs.clone(),
            output_schema: None,
            max_tokens: cfg.max_tokens,
            options: ProviderOptions {
                cache_prefix: true,
                ..ProviderOptions::default()
            },
        };
        let resp = match cfg.provider.complete(req).await {
            Ok(r) => r,
            Err(e) => return spend.report(None, &format!("error: {e}"), turn),
        };
        spend.charge(&resp);
        let calls: Vec<(String, String, serde_json::Value)> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();
        messages.push(Message {
            role: Role::Assistant,
            content: if resp.content.is_empty() {
                vec![ContentBlock::Text {
                    text: "(empty reply)".into(),
                }]
            } else {
                resp.content.clone()
            },
        });
        if calls.is_empty() {
            messages.push(Message::user(format!(
                "Call a tool, or {FINAL_TOOL} with your answer."
            )));
            continue;
        }
        let mut results = vec![];
        for (id, name, input) in calls {
            if name == FINAL_TOOL {
                match check_fails(cfg, tools, ctx).await {
                    Some(refusal) => results.push(ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: refusal,
                        is_error: true,
                    }),
                    None => {
                        let columns = input
                            .get("columns")
                            .and_then(|c| c.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let rows = input
                            .get("rows")
                            .and_then(|r| r.as_array())
                            .cloned()
                            .unwrap_or_default();
                        return spend.report(Some(relation(&columns, &rows)), "final", turn);
                    }
                }
                continue;
            }
            let args = json_args(input.get("args"));
            let (content, is_error) = call_tool(tools, ctx, &name, &args).await;
            results.push(ContentBlock::ToolResult {
                tool_use_id: id,
                content,
                is_error,
            });
        }
        messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
    spend.report(None, "turns_exhausted", cfg.max_turns)
}

async fn run_json(
    cfg: &PlainConfig,
    tools: &ToolRegistry,
    ctx: &ToolContext,
    task_text: String,
) -> PlainReport {
    let mut system = system_prompt(tools);
    if let Some(c) = &cfg.check {
        system.push_str(&format!(
            "\nA final is accepted only when `{c}` exits 0 in the workspace; run it yourself first.\n"
        ));
    }
    let mut messages = vec![Message::user(task_text)];
    let mut spend = Spend {
        usage: BudgetUsage::default(),
        started: std::time::Instant::now(),
    };
    for turn in 1..=cfg.max_turns {
        if !spend.affords(&cfg.budget) {
            return spend.report(None, "budget_exhausted", turn - 1);
        }
        let req = CompletionRequest {
            alias: kleene_core::ModelAlias(cfg.alias.clone()),
            model: cfg.alias.clone(),
            system: system.clone(),
            messages: messages.clone(),
            tools: vec![],
            output_schema: None,
            max_tokens: cfg.max_tokens,
            options: ProviderOptions {
                cache_prefix: true,
                ..ProviderOptions::default()
            },
        };
        let resp = match cfg.provider.complete(req).await {
            Ok(r) => r,
            Err(e) => return spend.report(None, &format!("error: {e}"), turn),
        };
        spend.charge(&resp);
        let text = resp.text();
        messages.push(Message::assistant(text.clone()));
        let action: serde_json::Value = match extract_json(&text)
            .and_then(|j| serde_json::from_str(&j).ok())
        {
            Some(a) => a,
            None => {
                messages.push(Message::user(
                    "That was not a single JSON object. Reply with {\"tool\": ..., \"args\": [...]} or {\"final\": [[...]], \"columns\": [...]}.",
                ));
                continue;
            }
        };
        if let Some(rows) = action.get("final").and_then(|f| f.as_array()) {
            if let Some(refusal) = check_fails(cfg, tools, ctx).await {
                messages.push(Message::user(refusal));
                continue;
            }
            let columns = action
                .get("columns")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            return spend.report(Some(relation(&columns, rows)), "final", turn);
        }
        let name = action.get("tool").and_then(|t| t.as_str()).unwrap_or("");
        let args = json_args(action.get("args"));
        let (feedback, _) = call_tool(tools, ctx, name, &args).await;
        messages.push(Message::user(feedback));
    }
    spend.report(None, "turns_exhausted", cfg.max_turns)
}

/// The first balanced JSON object in `text`. A model that writes several
/// actions in one reply (Haiku does, with imagined tool output between
/// them) gets its first action executed and the real output fed back,
/// instead of a parse error for the whole reply.
fn extract_json(text: &str) -> Option<String> {
    let t = text.trim();
    let start = t.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in t[start..].char_indices() {
        if in_string {
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(t[start..start + i + c.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::extract_json;

    #[test]
    fn a_single_object_is_taken_whole() {
        let t = "Sure.\n{\"tool\": \"read\", \"args\": [\"a.py\"]}\n";
        assert_eq!(
            extract_json(t).as_deref(),
            Some("{\"tool\": \"read\", \"args\": [\"a.py\"]}")
        );
    }

    #[test]
    fn the_first_of_several_objects_is_taken() {
        let t = "{\"tool\": \"files\", \"args\": [\"*\"]}\nimagined output\n{\"tool\": \"read\", \"args\": [\"x\"]}";
        assert_eq!(
            extract_json(t).as_deref(),
            Some("{\"tool\": \"files\", \"args\": [\"*\"]}")
        );
    }

    #[test]
    fn braces_inside_strings_do_not_close_the_object() {
        let t =
            "{\"tool\": \"write_file\", \"args\": [\"a.py\", \"d = {\\\"k\\\": 1}\\n\"]} trailing";
        let got = extract_json(t).unwrap();
        assert!(got.ends_with("]}"), "{got}");
        assert!(serde_json::from_str::<serde_json::Value>(&got).is_ok());
    }

    #[test]
    fn an_unbalanced_object_is_none() {
        assert_eq!(extract_json("{\"tool\": \"x\", \"args\": ["), None);
        assert_eq!(extract_json("no json here"), None);
    }
}
