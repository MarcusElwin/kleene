//! The plain-agent baseline: a tool-calling loop on the same provider,
//! tools and budgets as the harness, without SQL. Each turn the model
//! returns one JSON action, `{"tool": name, "args": [...]}` or
//! `{"final": [[cell, ...], ...], "columns": [...]}`; tool output is fed back
//! as text. This is the control the RLM-paper framing needs: same model,
//! same client, same spend, only the abstraction differs.

use kleene_core::{Batch, Budget, BudgetUsage, DataType, Field, Schema, Value};
use kleene_llm::{CompletionRequest, Message, Provider, ProviderOptions};
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

/// Run one task with the plain agent.
pub async fn run(cfg: &PlainConfig, task: &str, context: Option<&str>) -> PlainReport {
    let tools = kleene_tools::standard_tools();
    let ctx = ToolContext::new(cfg.workspace.clone());
    let system = system_prompt(&tools);
    let mut messages = vec![Message::user(match context {
        Some(c) => format!("# Task\n{task}\n\n# Context\n{c}"),
        None => format!("# Task\n{task}"),
    })];
    let mut usage = BudgetUsage::default();
    let started = std::time::Instant::now();
    for turn in 1..=cfg.max_turns {
        let mut next = usage;
        next.calls += 1;
        if cfg.budget.check(&next).is_err() {
            return PlainReport {
                answer: None,
                outcome: "budget_exhausted".into(),
                turns: turn - 1,
                usage,
            };
        }
        let req = CompletionRequest {
            alias: kleene_core::ModelAlias(cfg.alias.clone()),
            model: cfg.alias.clone(),
            system: system.clone(),
            messages: messages.clone(),
            tools: vec![],
            output_schema: None,
            max_tokens: cfg.max_tokens,
            options: ProviderOptions::default(),
        };
        let resp = match cfg.provider.complete(req).await {
            Ok(r) => r,
            Err(e) => {
                usage.wall = started.elapsed();
                return PlainReport {
                    answer: None,
                    outcome: format!("error: {e}"),
                    turns: turn,
                    usage,
                };
            }
        };
        usage.calls += 1;
        usage.tokens += resp.usage.total_tokens();
        usage.dollars += resp.usage.cost_usd.unwrap_or(0.0);
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
            let columns: Vec<String> = action
                .get("columns")
                .and_then(|c| c.as_array())
                .map(|c| {
                    c.iter()
                        .map(|v| v.as_str().unwrap_or("col").to_string())
                        .collect()
                })
                .unwrap_or_default();
            let width = rows
                .iter()
                .filter_map(|r| r.as_array())
                .map(|r| r.len())
                .max()
                .unwrap_or(columns.len().max(1));
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
            usage.wall = started.elapsed();
            return PlainReport {
                answer: Some(Batch {
                    schema: Arc::new(Schema::new(fields)),
                    rows,
                }),
                outcome: "final".into(),
                turns: turn,
                usage,
            };
        }
        let name = action.get("tool").and_then(|t| t.as_str()).unwrap_or("");
        let args: Vec<Value> = action
            .get("args")
            .and_then(|a| a.as_array())
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
            .unwrap_or_default();
        let feedback = match tools.get(name) {
            None => format!(
                "unknown tool {name:?}; tools: {}",
                tools
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(tool) => match tool.call(&args, &ctx).await {
                Ok(batch) => format!("{}\n{} row(s)", batch.render_table(40), batch.rows.len()),
                Err(e) => format!("error: {e}"),
            },
        };
        messages.push(Message::user(feedback));
    }
    usage.wall = started.elapsed();
    PlainReport {
        answer: None,
        outcome: "turns_exhausted".into(),
        turns: cfg.max_turns,
        usage,
    }
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
