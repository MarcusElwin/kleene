//! What the model reads: the cached system prefix (rules, catalog, role,
//! budget) and the rendering of results back into the transcript.
//!
//! Everything here is part of the model-facing interface: keep it stable and
//! tested, and keep [`system_prompt`] deterministic for the same inputs so
//! providers can cache it across turns.

use crate::{AgentRole, Rendered};
use kleene_core::{
    Budget, BudgetUsage, CallKind, Catalog, FunctionDef, FunctionReturn, TableDef, Volatility,
};
use std::fmt::Write as _;

/// Inputs of the system prefix.
pub struct PromptContext<'a> {
    /// What the session can see.
    pub catalog: &'a Catalog,
    /// The session's role.
    pub role: &'a AgentRole,
    /// Declared agents available to `spawn`.
    pub agents: &'a [AgentRole],
    /// Limits.
    pub budget: &'a Budget,
    /// This session's depth.
    pub depth: u32,
    /// Deepest allowed session.
    pub max_depth: u32,
    /// Turn cap.
    pub max_turns: u32,
    /// Learned playbook entries for this task's kind (may be empty).
    pub playbook: &'a [crate::PlaybookExample],
}

/// Size of a preloaded context, for the task message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSummary {
    /// Rows in `ctx`.
    pub rows: u64,
    /// Characters across all rows.
    pub chars: u64,
    /// Digest of the context text, so two sessions with the same task over
    /// different contexts never share a memo entry or a cached transcript.
    pub digest: u64,
}

impl ContextSummary {
    /// Summarise a context from its paragraphs.
    pub fn of(text: &str, rows: u64) -> Self {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(text.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash[..8]);
        Self {
            rows,
            chars: text.chars().count() as u64,
            digest: u64::from_be_bytes(bytes),
        }
    }
}

/// The system prefix: rules, catalog, agents, budget, an example and the
/// role's own prompt.
pub fn system_prompt(cx: &PromptContext<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "You are a session inside Kleene, a SQL engine whose functions can call language models and tools. \
Everything you do is CallSQL: reply with one or more statements inside a single ```sql fence and nothing else. \
You will see the rendered results and reply again, until you end with FINAL. \
This session runs at depth {} of {}; you have at most {} turns.\n",
        cx.depth, cx.max_depth, cx.max_turns
    );
    out.push_str(RULES);
    out.push('\n');
    render_catalog(&mut out, cx.catalog);
    if !cx.agents.is_empty() {
        out.push_str("## Declared agents (for spawn)\n");
        let mut agents: Vec<&AgentRole> = cx.agents.iter().collect();
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        for a in agents {
            let tools = if a.tools.is_empty() {
                "all tools".to_string()
            } else {
                format!("tools ({})", a.tools.join(", "))
            };
            let _ = writeln!(
                out,
                "- {}: model {}, effort {}, {}, budget {}{}",
                a.name,
                a.model.0,
                a.effort.as_deref().unwrap_or("default"),
                tools,
                render_budget(&a.budget),
                if a.prompt.is_empty() {
                    String::new()
                } else {
                    format!(" -- {}", first_line(&a.prompt))
                }
            );
        }
        out.push('\n');
    }
    let _ = writeln!(out, "## Budget\n{}\n", render_budget(cx.budget));
    out.push_str(EXAMPLE);
    if !cx.playbook.is_empty() {
        out.push_str("\n## Learned playbook (SQL that solved earlier tasks of this kind; adapt, do not copy blindly)\n");
        for p in cx.playbook {
            let _ = writeln!(
                out,
                "-- {} (v{}, {}/{} wins)\n```sql\n{}\n```",
                p.kind,
                p.version,
                p.wins,
                p.tries,
                p.sql.trim()
            );
        }
    }
    if !cx.role.prompt.is_empty() {
        let _ = write!(out, "\n## Your role\n{}\n", cx.role.prompt.trim());
    }
    out
}

const RULES: &str = "## CallSQL rules
- PostgreSQL-flavoured subset: SELECT, JOIN ... ON, WHERE, GROUP BY/HAVING, ORDER BY, LIMIT, subqueries, EXISTS, IN, WITH and WITH RECURSIVE; CREATE TABLE t AS SELECT ...; INSERT INTO t SELECT ...; DROP TABLE t.
- Define judgement functions once, then use them in predicates: CREATE FUNCTION verify(x TEXT) RETURNS BOOLEAN AS PROMPT 'Is {x} ...? ' (also AS SQL (SELECT ...) and AS SHELL 'cmd {x}').
- Model functions: llm(prompt), llm_bool(prompt), llm_json(prompt, schema), expand(prompt, n) as a table. Each distinct argument tuple costs one call; repeated calls are memoised, so express work as sets, not loops.
- Tools marked read-only are table functions you use in FROM (files, grep, lines, read, chunks, git_log, ...). Tools with side effects run only as statements: CALL tool(args) or CALL tool(args) FROM query (one call per row, in order); they cannot appear inside expressions.
- Delegate with rlm(question, context) in CROSS JOIN LATERAL to run a child session over each partition, or spawn(agent, task, context) to run a declared agent (CREATE AGENT name MODEL 'worker' EFFORT 'low' TOOLS (files, grep) BUDGET (calls 40) PROMPT '...'). A child sees its context as table ctx(text) and returns answer (first column of its FINAL) and detail (its FINAL row as JSON). Children are refused at the maximum depth.
- EXPLAIN SELECT ... shows estimated rows, calls, tokens and dollars before you spend them, the join orders considered and the alternatives priced; EXPLAIN ANALYZE also runs it. A statement estimated above the remaining budget is refused with its plan; narrow it and retry.
- The planner reorders joins around call predicates, pushes cheap conjuncts first and turns NOT EXISTS into semi-joins. Give an expensive predicate a cheap proxy and it cascades: CREATE FUNCTION score(t TEXT) RETURNS DOUBLE AS PROMPT '...' MODEL 'proxy' (a score in [0, 1]), then CREATE FUNCTION relevant(t TEXT) RETURNS BOOLEAN AS PROMPT '...' PROXY score THRESHOLDS (0.2, 0.8); only the band between the thresholds pays for the real call.
- In WITH RECURSIVE, a trailing ORDER BY score LIMIT k keeps the best k new rows of each round (beam search); SET max_recursion_rounds bounds depth.
- SET budget.calls = n, SET effort = 'low', SET model.default = 'worker' change this session's settings.
- Results are truncated to the first rows: aggregate, filter and LIMIT deliberately; never page through a large relation row by row. Peek (COUNT, MIN, MAX, a LIMIT 3 sample), partition (chunks, files), locate (grep, lines), map (LATERAL rlm), then reduce.
- Errors come back as text with a hint; fix the statement and retry. A failed statement stops the rest of that reply.
- End with FINAL(expr) or FINAL FROM (SELECT ...): its rows are your answer and the session ends. Make the answer a small, well-named relation.
";

const EXAMPLE: &str = "## Example
```sql
SELECT COUNT(*) AS rows, MIN(ordinal), MAX(ordinal) FROM ctx;
```
```sql
CREATE FUNCTION verify(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is the claim ''{c}'' supported by the context? Answer yes or no.';
CREATE FUNCTION refute(c TEXT, ce TEXT) RETURNS BOOLEAN AS PROMPT 'Does ''{ce}'' contradict ''{c}''?';
EXPLAIN SELECT candidate FROM candidates WHERE verify(candidate) AND NOT EXISTS (SELECT 1 FROM counterexamples ce WHERE refute(candidate, ce.text));
```
```sql
FINAL FROM (SELECT candidate FROM candidates WHERE verify(candidate) AND NOT EXISTS (SELECT 1 FROM counterexamples ce WHERE refute(candidate, ce.text)));
```
";

/// A predicate selecting the functions of one catalog heading.
type Group = fn(&FunctionDef) -> bool;

fn render_catalog(out: &mut String, catalog: &Catalog) {
    out.push_str("## Catalog\n### Tables\n");
    let mut tables: Vec<_> = catalog
        .tables()
        .filter(|t| !is_hidden_table(&t.name))
        .collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    let (own, trace): (Vec<&TableDef>, Vec<&TableDef>) = tables
        .into_iter()
        .partition(|t| !t.name.starts_with("trace_"));
    if own.is_empty() {
        out.push_str("(no tables yet; CREATE TABLE ... AS SELECT to make some)\n");
    }
    for t in own {
        let cols: Vec<String> = t
            .schema
            .fields
            .iter()
            .map(|f| format!("{} {}", f.name, f.data_type))
            .collect();
        let _ = writeln!(
            out,
            "- {}({}){}",
            t.name,
            cols.join(", "),
            desc(&t.description)
        );
    }
    if !trace.is_empty() {
        out.push_str("Your own trace, live: ");
        let names: Vec<&str> = trace.iter().map(|t| t.name.as_str()).collect();
        out.push_str(&names.join(", "));
        out.push('\n');
    }
    let mut funcs: Vec<&FunctionDef> = catalog.functions().collect();
    funcs.sort_by(|a, b| a.name.cmp(&b.name));
    let groups: [(&str, Group); 6] = [
        ("### Model calls", |f| {
            matches!(
                &f.call_kind,
                CallKind::LlmScalar { alias } | CallKind::LlmTable { alias } if !alias.is_jev()
            )
        }),
        ("### Typed decisions (Jev: a probability, a label or a score, far cheaper than a model call)", |f| {
            matches!(
                &f.call_kind,
                CallKind::LlmScalar { alias } | CallKind::LlmTable { alias } if alias.is_jev()
            )
        }),
        ("### Delegation", |f| {
            matches!(f.call_kind, CallKind::Recursive { .. })
        }),
        ("### Tools, read-only (use in FROM)", |f| {
            matches!(f.call_kind, CallKind::Tool { .. }) && f.volatility != Volatility::Volatile
        }),
        ("### Tools with side effects (CALL only)", |f| {
            matches!(f.call_kind, CallKind::Tool { .. }) && f.volatility == Volatility::Volatile
        }),
        ("### Functions", |f| matches!(f.call_kind, CallKind::Pure)),
    ];
    for (heading, pred) in groups {
        let members: Vec<&&FunctionDef> = funcs.iter().filter(|f| pred(f)).collect();
        if members.is_empty() {
            continue;
        }
        out.push_str(heading);
        out.push('\n');
        if heading == "### Functions" {
            // Pure functions are familiar; names are enough.
            let names: Vec<&str> = members.iter().map(|f| f.name.as_str()).collect();
            out.push_str(&names.join(", "));
            out.push('\n');
            continue;
        }
        for f in members {
            let _ = writeln!(out, "- {}{}", signature(f), desc(&f.description));
        }
    }
    out.push('\n');
}

fn is_hidden_table(name: &str) -> bool {
    name == "memo" || name.starts_with("kleene_") || name.starts_with("cgs_")
}

fn desc(d: &str) -> String {
    if d.is_empty() {
        String::new()
    } else {
        format!("  -- {}", first_line(d))
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or_default().trim();
    if l.chars().count() > 100 {
        format!("{}…", l.chars().take(100).collect::<String>())
    } else {
        l.to_string()
    }
}

/// `name(TYPE, TYPE, ...) -> TYPE` or `-> TABLE(col TYPE, ...)`.
pub fn signature(f: &FunctionDef) -> String {
    let mut args: Vec<String> = f.args.iter().map(|t| t.to_string()).collect();
    if f.variadic {
        args.push("...".into());
    }
    let ret = match &f.returns {
        FunctionReturn::Scalar { data_type } => data_type.to_string(),
        FunctionReturn::Table { schema } => format!(
            "TABLE({})",
            schema
                .fields
                .iter()
                .map(|c| format!("{} {}", c.name, c.data_type))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    format!("{}({}) -> {}", f.name, args.join(", "), ret)
}

/// `calls 200, tokens 60000, dollars 1.50, depth 2, wall 300s`, or a note
/// that the budget is unbounded.
pub fn render_budget(b: &Budget) -> String {
    let mut parts = vec![];
    if let Some(c) = b.calls {
        parts.push(format!("calls {c}"));
    }
    if let Some(t) = b.tokens {
        parts.push(format!("tokens {t}"));
    }
    if let Some(d) = b.dollars {
        parts.push(format!("dollars {d:.2}"));
    }
    if let Some(d) = b.max_depth {
        parts.push(format!("depth {d}"));
    }
    if let Some(w) = b.wall {
        parts.push(format!("wall {}s", w.as_secs()));
    }
    if parts.is_empty() {
        "unbounded (be economical anyway)".into()
    } else {
        parts.join(", ")
    }
}

/// The first user turn: the task, the context summary, the ask.
pub fn task_message(task: &str, context: Option<&ContextSummary>) -> String {
    let mut out = String::new();
    out.push_str("# Task\n");
    out.push_str(task.trim());
    out.push('\n');
    if let Some(c) = context {
        let _ = write!(
            out,
            "\nThe context is loaded as table ctx(ordinal, text): {} row{}, about {} tokens, digest {:016x}. Peek and partition it; do not SELECT it whole.\n",
            c.rows,
            if c.rows == 1 { "" } else { "s" },
            c.chars / 4,
            c.digest
        );
    }
    out.push_str("\nReply with CallSQL in a ```sql fence.");
    out
}

/// The SQL in a model reply: every fenced block tagged `sql`, `callsql` or
/// untagged, joined; or the whole reply if it plainly starts with a
/// statement; or `None` for prose.
pub fn extract_sql(reply: &str) -> Option<String> {
    let text = reply.replace("\r\n", "\n");
    let mut blocks = vec![];
    let mut rest = text.as_str();
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let (tag, body_start) = match after.find('\n') {
            Some(nl) => (after[..nl].trim().to_ascii_lowercase(), nl + 1),
            None => break,
        };
        let body = &after[body_start..];
        let (block, next) = match body.find("```") {
            Some(end) => (&body[..end], &body[end + 3..]),
            None => (body, ""),
        };
        let tag_word = tag.split_whitespace().next().unwrap_or_default();
        if matches!(tag_word, "" | "sql" | "callsql" | "postgresql" | "psql") {
            let b = block.trim();
            if !b.is_empty() {
                blocks.push(b.to_string());
            }
        }
        rest = next;
    }
    if !blocks.is_empty() {
        return Some(blocks.join("\n"));
    }
    let trimmed = text.trim();
    let first = trimmed
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        first.as_str(),
        "SELECT" | "WITH" | "CREATE" | "INSERT" | "DROP" | "CALL" | "SET" | "EXPLAIN" | "FINAL"
    ) {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// The user turn after executing a reply: each statement's rendering, a
/// note when one failed, and a budget footer.
pub fn render_results(
    results: &[Rendered],
    turn: u32,
    max_turns: u32,
    used: &BudgetUsage,
    remaining: &Budget,
) -> String {
    let mut out = String::new();
    for r in results {
        out.push_str(r.text.trim_end());
        out.push('\n');
        if r.is_error {
            out.push_str("Statement failed; later statements in that reply were not run.\n");
        }
        out.push('\n');
    }
    let _ = write!(
        out,
        "turn {turn}/{max_turns}; used {} calls, {} tokens, ${:.4}; remaining: {}",
        used.calls,
        used.tokens,
        used.dollars,
        render_budget(remaining)
    );
    if !results.iter().any(|r| r.is_final) {
        out.push_str("\nContinue, or end with FINAL.");
    }
    out
}

/// A reminder when a reply carried no SQL.
pub fn nudge(reason: &str) -> String {
    format!("{reason} Reply with CallSQL inside a single ```sql fence, or FINAL(...) to finish.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::{
        standard_catalog, DataType, Field, ModelAlias, Schema, TableDef, TableSource,
    };

    fn catalog() -> Catalog {
        let mut c = standard_catalog();
        c.add_table(TableDef {
            name: "ctx".into(),
            schema: Schema::new(vec![Field::new("text", DataType::Text)]),
            source: TableSource::Stored,
            volatility: Volatility::Stable,
            description: "the context".into(),
        });
        c.add_table(TableDef {
            name: "trace_calls".into(),
            schema: Schema::new(vec![Field::new("call", DataType::Text)]),
            source: TableSource::Stored,
            volatility: Volatility::Stable,
            description: String::new(),
        });
        c.add_function(FunctionDef {
            name: "shell".into(),
            args: vec![DataType::Text],
            variadic: true,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![Field::new("stdout", DataType::Text)]),
            },
            call_kind: CallKind::Tool {
                tool: "shell".into(),
            },
            volatility: Volatility::Volatile,
            description: "run a command".into(),
        });
        c.add_function(FunctionDef {
            name: "grep".into(),
            args: vec![DataType::Text],
            variadic: true,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![Field::new("text", DataType::Text)]),
            },
            call_kind: CallKind::Tool {
                tool: "grep".into(),
            },
            volatility: Volatility::Stable,
            description: String::new(),
        });
        c
    }

    fn role() -> AgentRole {
        AgentRole {
            name: "reviewer".into(),
            model: ModelAlias::worker(),
            effort: Some("low".into()),
            tools: vec!["grep".into()],
            budget: Budget {
                calls: Some(40),
                ..Budget::unbounded()
            },
            prompt: "You review one hypothesis.".into(),
            max_turns: None,
        }
    }

    #[test]
    fn system_prompt_lists_catalog_by_group_and_is_deterministic() {
        let cat = catalog();
        let role = role();
        let agents = [role.clone()];
        let budget = Budget {
            calls: Some(200),
            dollars: Some(1.5),
            ..Budget::unbounded()
        };
        let cx = PromptContext {
            catalog: &cat,
            role: &role,
            agents: &agents,
            budget: &budget,
            depth: 1,
            max_depth: 2,
            max_turns: 12,
            playbook: &[],
        };
        let a = system_prompt(&cx);
        let b = system_prompt(&cx);
        assert_eq!(a, b);
        assert!(a.contains("depth 1 of 2"), "{a}");
        assert!(a.contains("- ctx(text TEXT)  -- the context"), "{a}");
        assert!(a.contains("Your own trace, live: trace_calls"), "{a}");
        let side = a.find("### Tools with side effects (CALL only)").unwrap();
        let shell = a.find("- shell(TEXT, ...) -> TABLE(stdout TEXT)").unwrap();
        assert!(shell > side, "{a}");
        let read = a.find("### Tools, read-only (use in FROM)").unwrap();
        let grep = a.find("- grep(TEXT, ...)").unwrap();
        assert!(grep > read && grep < side, "{a}");
        assert!(
            a.contains("### Delegation\n- rlm(TEXT, ...) -> TABLE(answer TEXT, detail JSON)"),
            "{a}"
        );
        assert!(a.contains("- reviewer: model worker, effort low, tools (grep), budget calls 40 -- You review one hypothesis."), "{a}");
        assert!(a.contains("## Budget\ncalls 200, dollars 1.50"), "{a}");
        assert!(
            a.trim_end()
                .ends_with("## Your role\nYou review one hypothesis."),
            "{a}"
        );
    }

    #[test]
    fn extract_sql_handles_fences_and_bare_statements() {
        assert_eq!(
            extract_sql("Sure.\n```sql\nSELECT 1;\n```\nthen\n```\nSELECT 2;\n```"),
            Some("SELECT 1;\nSELECT 2;".into())
        );
        assert_eq!(
            extract_sql("```SQL title\r\nSELECT 1\r\n```"),
            Some("SELECT 1".into())
        );
        assert_eq!(extract_sql("select 1 as x"), Some("select 1 as x".into()));
        assert_eq!(extract_sql("FINAL(42)"), Some("FINAL(42)".into()));
        assert_eq!(
            extract_sql("I think we should look at the files first."),
            None
        );
        assert_eq!(
            extract_sql("```python\nprint(1)\n```"),
            None,
            "non-SQL fences are ignored"
        );
        assert_eq!(
            extract_sql("```sql\nSELECT 3"),
            Some("SELECT 3".into()),
            "unterminated fence"
        );
    }

    #[test]
    fn results_and_task_messages() {
        let results = vec![
            Rendered {
                text: "x\n-\n1\n1 row".into(),
                is_error: false,
                is_final: false,
                rows: None,
            },
            Rendered {
                text: "error: nope\nhint: fix".into(),
                is_error: true,
                is_final: false,
                rows: None,
            },
        ];
        let used = BudgetUsage {
            calls: 3,
            tokens: 400,
            dollars: 0.0123,
            wall: Default::default(),
        };
        let remaining = Budget {
            calls: Some(17),
            ..Budget::unbounded()
        };
        let r = render_results(&results, 2, 30, &used, &remaining);
        assert!(
            r.contains("Statement failed; later statements in that reply were not run."),
            "{r}"
        );
        assert!(
            r.contains("turn 2/30; used 3 calls, 400 tokens, $0.0123; remaining: calls 17"),
            "{r}"
        );
        assert!(r.ends_with("Continue, or end with FINAL."), "{r}");
        let t = task_message(
            "Who wrote the memo?",
            Some(&ContextSummary {
                rows: 12,
                chars: 4000,
                digest: 0xabc,
            }),
        );
        assert!(
            t.contains("ctx(ordinal, text): 12 rows, about 1000 tokens, digest 0000000000000abc"),
            "{t}"
        );
        assert_ne!(
            ContextSummary::of("a", 1).digest,
            ContextSummary::of("b", 1).digest
        );
        assert!(task_message("q", None).ends_with("Reply with CallSQL in a ```sql fence."));
        assert!(nudge("No SQL found.").starts_with("No SQL found. Reply with CallSQL"));
    }
}
