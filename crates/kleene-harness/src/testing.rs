//! A deterministic provider for tests and demos without a model.

use kleene_llm::{
    Capabilities, CompletionRequest, CompletionResponse, ContentBlock, Provider, ProviderError,
    StopReason, Usage,
};
use std::sync::atomic::{AtomicU64, Ordering};

/// Answers from a list of `(needle, response)` rules matched against the
/// last user message; the first matching rule wins. Counts calls.
pub struct ScriptedProvider {
    rules: Vec<(String, String)>,
    fallback: String,
    calls: AtomicU64,
    /// The system prefix of the most recent request.
    last_system: std::sync::Mutex<String>,
}

impl ScriptedProvider {
    /// Build from rules; `fallback` answers when no rule matches.
    pub fn new(rules: Vec<(&str, &str)>, fallback: &str) -> Self {
        Self {
            rules: rules
                .into_iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            fallback: fallback.to_string(),
            calls: AtomicU64::new(0),
            last_system: std::sync::Mutex::new(String::new()),
        }
    }

    /// Calls served so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// The system prefix of the most recent request (a session's prompt).
    pub fn last_system(&self) -> String {
        self.last_system
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: false,
            tools: false,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: false,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut s) = self.last_system.lock() {
            s.clone_from(&req.system);
        }
        let prompt = req
            .messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = self
            .rules
            .iter()
            .find(|(needle, _)| prompt.contains(needle.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| self.fallback.clone());
        Ok(CompletionResponse {
            model: "scripted".into(),
            content: vec![ContentBlock::Text { text }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: prompt.len() as u64 / 4,
                output_tokens: 8,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(0.001),
            },
        })
    }
}

/// Like [`ScriptedProvider`] but with native tool calling: each rule's
/// response is a list of content blocks (text or `tool_use`), matched
/// against the text of the last user message, tool results included.
pub struct ScriptedToolProvider {
    rules: Vec<(String, Vec<ContentBlock>)>,
    fallback: Vec<ContentBlock>,
    calls: AtomicU64,
}

impl ScriptedToolProvider {
    /// Build from rules; `fallback` answers when no rule matches.
    pub fn new(rules: Vec<(&str, Vec<ContentBlock>)>, fallback: Vec<ContentBlock>) -> Self {
        Self {
            rules: rules.into_iter().map(|(a, b)| (a.to_string(), b)).collect(),
            fallback,
            calls: AtomicU64::new(0),
        }
    }

    /// Calls served so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// A `tool_use` block.
    pub fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedToolProvider {
    fn name(&self) -> &str {
        "scripted-tools"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: false,
            tools: true,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: false,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let prompt = req
            .messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .map(|c| match c {
                        ContentBlock::Text { text } => text.clone(),
                        ContentBlock::ToolResult { content, .. } => content.clone(),
                        ContentBlock::ToolUse { name, .. } => name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let content = self
            .rules
            .iter()
            .find(|(needle, _)| prompt.contains(needle.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| self.fallback.clone());
        let stop_reason = if content
            .iter()
            .any(|c| matches!(c, ContentBlock::ToolUse { .. }))
        {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(CompletionResponse {
            model: "scripted-tools".into(),
            content,
            stop_reason,
            usage: Usage {
                input_tokens: prompt.len() as u64 / 4,
                output_tokens: 8,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(0.001),
            },
        })
    }
}

/// A deterministic solver for the `nest` generator's tasks that delegates
/// every sub-unit to a child session with `rlm`, so a group `k` levels deep
/// runs sessions `k` deep. It reads the unit and the measure (spend, units
/// or levels) from the task text, writes the same CallSQL at every level,
/// and knows nothing about the answer: the oracle still judges it. Tests and
/// offline runs use it to show what the harness records as depth grows.
pub struct NestSolver {
    calls: AtomicU64,
}

impl Default for NestSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl NestSolver {
    /// A solver.
    pub fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
        }
    }

    /// Calls served so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// The first `unit <Word>-<digits>` in `text`, as the name alone.
    pub fn unit_named(text: &str) -> Option<String> {
        let mut rest = text;
        while let Some(i) = rest.find("unit ") {
            let after = &rest[i + 5..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if let Some((w, n)) = name.split_once('-') {
                if !w.is_empty()
                    && w.chars().all(|c| c.is_ascii_alphabetic())
                    && !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
                {
                    return Some(name);
                }
            }
            rest = after;
        }
        None
    }

    /// The CallSQL for one session: `unit`'s own figure for `measure`
    /// combined with what its included sub-units answer. Only the standard
    /// functions every session has: `split_part` does the parsing and
    /// `generate_series` the splitting of the sub-unit list.
    pub fn sql(unit: &str, measure: &str, pick_largest: bool) -> String {
        let quoted = unit.replace('\'', "''");
        // The unit's own contribution: its (corrected) direct spend, one
        // unit, or no levels of its own.
        let own = match measure {
            "units" => "SELECT 1 AS own FROM me".to_string(),
            "levels" => "SELECT 0 AS own FROM me".to_string(),
            _ => format!(
                "SELECT coalesce(c.v, d.v) AS own FROM (SELECT CAST(split_part(split_part(text, 'Direct spend this year: ', 2), ' kEUR', 1) AS BIGINT) AS v FROM me) d LEFT JOIN (SELECT CAST(split_part(split_part(text, 'should read ', 2), ' kEUR', 1) AS BIGINT) AS v FROM ctx WHERE starts_with(text, 'Correction: unit {quoted}''s ')) c ON true"
            ),
        };
        let total = if measure == "levels" {
            "SELECT CAST(coalesce(s.v + 1, 0) AS BIGINT) AS total FROM own o LEFT JOIN (SELECT MAX(v) AS v FROM sub) s ON true".to_string()
        } else {
            "SELECT CAST(o.own + coalesce(s.v, 0) AS BIGINT) AS total FROM own o LEFT JOIN (SELECT SUM(v) AS v FROM sub) s ON true".to_string()
        };
        let final_sql = if pick_largest {
            "FINAL FROM (SELECT kid AS unit, v AS total FROM sub ORDER BY v DESC, kid LIMIT 1)"
                .to_string()
        } else {
            "FINAL FROM (SELECT total FROM total)".to_string()
        };
        format!(
            "```sql\n\
CREATE TABLE me AS SELECT text FROM ctx WHERE starts_with(text, 'Unit {quoted} reports to ');\n\
CREATE TABLE kids AS SELECT trim(split_part(split_part(split_part(me.text, 'Sub-units: ', 2), '.', 1), ', ', g.generate_series)) AS kid FROM me CROSS JOIN generate_series(1, 3) AS g;\n\
CREATE TABLE included AS SELECT kid FROM kids WHERE kid <> '' AND kid <> (SELECT coalesce(split_part(split_part(text, 'Sub-unit ', 2), ' was divested', 1), '') FROM me);\n\
CREATE TABLE whole AS SELECT string_agg(text, '\n\n' ORDER BY ordinal) AS t FROM ctx;\n\
CREATE TABLE sub AS SELECT i.kid, CAST(CAST(r.answer AS DOUBLE) AS BIGINT) AS v FROM included i CROSS JOIN whole w CROSS JOIN LATERAL rlm(concat('Consolidate unit ', i.kid, ' for {measure}. Answer with FINAL over one row with column total.'), w.t) r;\n\
CREATE TABLE own AS {own};\n\
CREATE TABLE total AS {total};\n\
{final_sql}\n```"
        )
    }
}

#[async_trait::async_trait]
impl Provider for NestSolver {
    fn name(&self) -> &str {
        "nest-solver"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: false,
            tools: false,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: false,
            cost_reported: true,
        }
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let prompt = req
            .messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = match (prompt.contains("# Task"), Self::unit_named(&prompt)) {
            (true, Some(unit)) => {
                let measure = if prompt.contains(" for units.") || prompt.contains("How many units")
                {
                    "units"
                } else if prompt.contains(" for levels.") || prompt.contains("How many levels") {
                    "levels"
                } else {
                    "spend"
                };
                Self::sql(
                    &unit,
                    measure,
                    prompt.contains("largest consolidated spend"),
                )
            }
            // A second turn means the first statement failed; give up with
            // the feedback in the answer so the verdict shows it.
            _ => "```sql\nFINAL FROM (SELECT 'solver: a statement failed, see trace_statements' AS total)\n```".into(),
        };
        Ok(CompletionResponse {
            model: "nest-solver".into(),
            content: vec![ContentBlock::Text { text }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: prompt.len() as u64 / 4,
                output_tokens: 120,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(0.001),
            },
        })
    }
}
