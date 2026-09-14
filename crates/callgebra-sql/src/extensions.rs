//! CallSQL statements the SQL parser does not know: `CREATE FUNCTION ... AS
//! PROMPT | SQL | SHELL` and `CALL tool(args) [FROM query]`.
//!
//! Both are recognised by a small hand-written scanner and then delegated to
//! the ordinary planner for anything that is plain SQL (argument
//! expressions, the `FROM` query).

use crate::error::SqlError;
use crate::plan::LogicalPlan;
use crate::statement::{FunctionBody, Statement, StatementKind};
use callgebra_core::{CallKind, Catalog, DataType, Volatility};

fn unsupported(construct: &str, hint: &str) -> SqlError {
    SqlError::Unsupported {
        construct: construct.into(),
        hint: hint.into(),
    }
}

fn parse_err(message: impl Into<String>, hint: &str) -> SqlError {
    SqlError::Parse {
        message: message.into(),
        hint: Some(hint.into()),
    }
}

/// Try the extension statements; `Ok(None)` means "not one of ours".
pub(crate) fn plan_extension(text: &str, catalog: &Catalog) -> Result<Option<Statement>, SqlError> {
    let words: Vec<String> = text
        .split_whitespace()
        .take(4)
        .map(|w| w.to_ascii_uppercase())
        .collect();
    let head = words.join(" ");
    if head.starts_with("CREATE FUNCTION") || head.starts_with("CREATE OR REPLACE FUNCTION") {
        return plan_create_function(text).map(Some);
    }
    if head.starts_with("CREATE AGENT") || head.starts_with("CREATE OR REPLACE AGENT") {
        return plan_create_agent(text).map(Some);
    }
    if words.first().is_some_and(|w| w == "CALL") {
        return plan_call(text, catalog).map(Some);
    }
    Ok(None)
}

/// A cursor over statement text.
struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_ws(&mut self) {
        let trimmed = self.rest().trim_start();
        self.pos = self.src.len() - trimmed.len();
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        self.skip_ws();
        let rest = self.rest();
        if rest.len() >= kw.len() && rest[..kw.len()].eq_ignore_ascii_case(kw) {
            let after = &rest[kw.len()..];
            if after.is_empty() || !after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                self.pos += kw.len();
                return true;
            }
        }
        false
    }

    fn ident(&mut self) -> Option<String> {
        self.skip_ws();
        let rest = self.rest();
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        self.pos += end;
        Some(rest[..end].to_string())
    }

    /// A `'...'` string with `''` escapes.
    fn quoted(&mut self) -> Option<String> {
        self.skip_ws();
        let rest = self.rest();
        let mut chars = rest.char_indices();
        match chars.next() {
            Some((_, '\'')) => {}
            _ => return None,
        }
        let mut out = String::new();
        loop {
            match chars.next() {
                None => return None,
                Some((i, '\'')) => {
                    if rest[i + 1..].starts_with('\'') {
                        out.push('\'');
                        chars.next();
                    } else {
                        self.pos += i + 1;
                        return Some(out);
                    }
                }
                Some((_, c)) => out.push(c),
            }
        }
    }

    /// Text inside balanced parentheses, starting at `(`.
    fn parenthesised(&mut self) -> Option<String> {
        self.skip_ws();
        let rest = self.rest();
        if !rest.starts_with('(') {
            return None;
        }
        let mut depth = 0i32;
        let mut in_str = false;
        for (i, c) in rest.char_indices() {
            match c {
                '\'' => in_str = !in_str,
                '(' if !in_str => depth += 1,
                ')' if !in_str => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos += i + 1;
                        return Some(rest[1..i].to_string());
                    }
                }
                _ => {}
            }
        }
        None
    }
}

fn data_type(word: &str) -> Result<DataType, SqlError> {
    Ok(match word.to_ascii_uppercase().as_str() {
        "BOOLEAN" | "BOOL" => DataType::Bool,
        "BIGINT" | "INTEGER" | "INT" => DataType::Int,
        "DOUBLE" | "FLOAT" | "REAL" => DataType::Float,
        "TEXT" | "VARCHAR" | "STRING" => DataType::Text,
        "JSON" => DataType::Json,
        "VECTOR" => DataType::Vector,
        other => {
            return Err(parse_err(
                format!("unknown type {other}"),
                "types: BOOLEAN, BIGINT, DOUBLE, TEXT, JSON, VECTOR",
            ))
        }
    })
}

/// Plan `CREATE [OR REPLACE] AGENT name [MODEL 'alias'] [EFFORT 'level']
/// [TOOLS (a, b, ...)] [BUDGET (calls n, tokens n, dollars x, depth n)]
/// [PROMPT '...']`. Clauses are optional and may come in any order.
pub fn plan_create_agent(text: &str) -> Result<Statement, SqlError> {
    const HINT: &str = "CREATE AGENT reviewer MODEL 'worker' EFFORT 'low' TOOLS (files, grep) BUDGET (calls 40, tokens 60000) PROMPT 'You review one hypothesis.'";
    let mut sc = Scanner::new(text.trim().trim_end_matches(';'));
    if !sc.eat_keyword("CREATE") {
        return Err(parse_err("expected CREATE", HINT));
    }
    let replace = sc.eat_keyword("OR") && sc.eat_keyword("REPLACE");
    if !sc.eat_keyword("AGENT") {
        return Err(parse_err("expected AGENT", HINT));
    }
    let name = sc
        .ident()
        .ok_or_else(|| parse_err("expected an agent name", HINT))?;
    let mut model = "worker".to_string();
    let mut effort = None;
    let mut tools = vec![];
    let mut budget = vec![];
    let mut prompt = String::new();
    loop {
        sc.skip_ws();
        if sc.rest().is_empty() {
            break;
        }
        if sc.eat_keyword("MODEL") {
            model = sc
                .quoted()
                .ok_or_else(|| parse_err("MODEL expects a quoted alias", HINT))?;
        } else if sc.eat_keyword("EFFORT") {
            let e = sc
                .quoted()
                .ok_or_else(|| parse_err("EFFORT expects a quoted level", HINT))?;
            let lower = e.to_ascii_lowercase();
            if !matches!(lower.as_str(), "low" | "medium" | "high" | "max") {
                return Err(parse_err(
                    format!("unknown effort level {e}"),
                    "EFFORT 'low', 'medium' or 'high'",
                ));
            }
            effort = Some(lower);
        } else if sc.eat_keyword("TOOLS") {
            let list = sc
                .parenthesised()
                .ok_or_else(|| parse_err("TOOLS expects a parenthesised list", HINT))?;
            tools = list
                .split(',')
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
        } else if sc.eat_keyword("BUDGET") {
            let list = sc
                .parenthesised()
                .ok_or_else(|| parse_err("BUDGET expects a parenthesised list", HINT))?;
            for part in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let mut it = part.split_whitespace();
                let key = it.next().unwrap_or_default().to_ascii_lowercase();
                if !matches!(
                    key.as_str(),
                    "calls" | "tokens" | "dollars" | "depth" | "turns"
                ) {
                    return Err(parse_err(
                        format!("unknown budget dimension {key}"),
                        "BUDGET (calls n, tokens n, dollars x, depth n, turns n)",
                    ));
                }
                let value: f64 = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| parse_err(format!("budget {key} needs a number"), HINT))?;
                if it.next().is_some() {
                    return Err(parse_err(format!("budget {key}: unexpected text"), HINT));
                }
                budget.push((key, value));
            }
        } else if sc.eat_keyword("PROMPT") {
            prompt = sc
                .quoted()
                .ok_or_else(|| parse_err("PROMPT expects a quoted text", HINT))?;
        } else {
            return Err(parse_err(
                format!(
                    "unexpected clause: {}",
                    sc.rest().chars().take(30).collect::<String>()
                ),
                HINT,
            ));
        }
    }
    Ok(Statement {
        sql: text.trim().to_string(),
        kind: StatementKind::CreateAgent {
            name,
            model,
            effort,
            tools,
            budget,
            prompt,
            replace,
        },
    })
}

/// Plan `CREATE [OR REPLACE] FUNCTION name(arg TYPE, ...) RETURNS TYPE AS
/// PROMPT '...' | AS SQL (...) | AS SHELL '...' [IMMUTABLE | STABLE | VOLATILE]`.
pub fn plan_create_function(text: &str) -> Result<Statement, SqlError> {
    const HINT: &str =
        "CREATE FUNCTION name(x TEXT) RETURNS BOOLEAN AS PROMPT 'Is {x} valid? Answer yes or no.'";
    let mut sc = Scanner::new(text.trim().trim_end_matches(';'));
    if !sc.eat_keyword("CREATE") {
        return Err(parse_err("expected CREATE", HINT));
    }
    let replace = sc.eat_keyword("OR") && sc.eat_keyword("REPLACE");
    if !sc.eat_keyword("FUNCTION") {
        return Err(parse_err("expected FUNCTION", HINT));
    }
    let name = sc
        .ident()
        .ok_or_else(|| parse_err("expected a function name", HINT))?;
    let arg_text = sc
        .parenthesised()
        .ok_or_else(|| parse_err("expected an argument list in parentheses", HINT))?;
    let mut args = vec![];
    for part in arg_text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let mut it = part.split_whitespace();
        let aname = it.next().unwrap_or_default().to_string();
        let ty = it
            .next()
            .ok_or_else(|| parse_err(format!("argument {aname} needs a type"), HINT))?;
        args.push((aname, data_type(ty)?));
    }
    if !sc.eat_keyword("RETURNS") {
        return Err(parse_err("expected RETURNS", HINT));
    }
    let ret = sc
        .ident()
        .ok_or_else(|| parse_err("expected a return type", HINT))?;
    let returns = data_type(&ret)?;
    if !sc.eat_keyword("AS") {
        return Err(parse_err("expected AS", HINT));
    }
    let (body, default_vol) = if sc.eat_keyword("PROMPT") {
        let t = sc
            .quoted()
            .ok_or_else(|| parse_err("expected a quoted prompt template", HINT))?;
        (FunctionBody::Prompt { template: t }, Volatility::Immutable)
    } else if sc.eat_keyword("SQL") {
        let q = sc
            .parenthesised()
            .ok_or_else(|| parse_err("expected a parenthesised query", "AS SQL (SELECT ...)"))?;
        (
            FunctionBody::Sql {
                query: q.trim().to_string(),
            },
            Volatility::Stable,
        )
    } else if sc.eat_keyword("SHELL") {
        let c = sc.quoted().ok_or_else(|| {
            parse_err(
                "expected a quoted command",
                "AS SHELL 'python check.py {x}'",
            )
        })?;
        (FunctionBody::Shell { command: c }, Volatility::Volatile)
    } else {
        return Err(parse_err("expected PROMPT, SQL or SHELL after AS", HINT));
    };
    let mut volatility = default_vol;
    let mut proxy = None;
    let mut model = None;
    loop {
        if sc.eat_keyword("MODEL") {
            model = Some(sc.quoted().ok_or_else(|| {
                parse_err("expected a quoted model alias after MODEL", "MODEL 'proxy'")
            })?);
        } else if sc.eat_keyword("IMMUTABLE") {
            volatility = Volatility::Immutable;
        } else if sc.eat_keyword("STABLE") {
            volatility = Volatility::Stable;
        } else if sc.eat_keyword("VOLATILE") {
            volatility = Volatility::Volatile;
        } else if sc.eat_keyword("PROXY") {
            const PHINT: &str = "PROXY score_fn THRESHOLDS (0.2, 0.8)";
            let pname = sc
                .ident()
                .ok_or_else(|| parse_err("expected a proxy function name", PHINT))?;
            let (low, high) = if sc.eat_keyword("THRESHOLDS") {
                let t = sc
                    .parenthesised()
                    .ok_or_else(|| parse_err("expected THRESHOLDS (low, high)", PHINT))?;
                let nums: Vec<f64> = t
                    .split(',')
                    .map(|x| x.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| parse_err("thresholds must be numbers", PHINT))?;
                if nums.len() != 2
                    || !(0.0..=1.0).contains(&nums[0])
                    || nums[0] > nums[1]
                    || nums[1] > 1.0
                {
                    return Err(parse_err("thresholds must be 0 <= low <= high <= 1", PHINT));
                }
                (nums[0], nums[1])
            } else {
                (0.2, 0.8)
            };
            proxy = Some((pname, low, high));
        } else {
            break;
        }
    }
    sc.skip_ws();
    if !sc.rest().is_empty() {
        return Err(parse_err(
            format!(
                "unexpected trailing text: {}",
                sc.rest().chars().take(30).collect::<String>()
            ),
            HINT,
        ));
    }
    // Placeholders must name declared arguments.
    if let FunctionBody::Prompt { template } | FunctionBody::Shell { command: template } = &body {
        for ph in placeholders(template) {
            if !args.iter().any(|(a, _)| a.eq_ignore_ascii_case(&ph)) {
                return Err(parse_err(
                    format!("placeholder {{{ph}}} is not an argument of {name}"),
                    "every {placeholder} must match an argument name",
                ));
            }
        }
    }
    Ok(Statement {
        sql: text.trim().to_string(),
        kind: StatementKind::CreateFunction {
            name,
            args,
            returns,
            body,
            volatility,
            replace,
            proxy,
            model,
        },
    })
}

/// `{name}` placeholders in a template.
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push(name.to_string());
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// Plan `CALL tool(args) [FROM query]`.
///
/// The arguments are planned as a projection over the `FROM` query (or over
/// a single empty row), so they may reference its columns; the tool runs
/// once per input row, in order.
pub fn plan_call(text: &str, catalog: &Catalog) -> Result<Statement, SqlError> {
    const HINT: &str = "CALL shell('ls -la') or CALL write_file(path, body) FROM drafts";
    let trimmed = text.trim().trim_end_matches(';');
    let mut sc = Scanner::new(trimmed);
    if !sc.eat_keyword("CALL") {
        return Err(parse_err("expected CALL", HINT));
    }
    let tool = sc
        .ident()
        .ok_or_else(|| parse_err("expected a tool name after CALL", HINT))?;
    let arg_text = sc
        .parenthesised()
        .ok_or_else(|| parse_err("expected an argument list in parentheses", HINT))?;
    let from = if sc.eat_keyword("FROM") {
        sc.skip_ws();
        Some(sc.rest().to_string())
    } else {
        sc.skip_ws();
        if !sc.rest().is_empty() {
            return Err(parse_err(
                format!(
                    "unexpected text after CALL: {}",
                    sc.rest().chars().take(30).collect::<String>()
                ),
                HINT,
            ));
        }
        None
    };
    let def = catalog
        .function(&tool)
        .ok_or_else(|| SqlError::Unresolved {
            what: "function",
            name: tool.clone(),
            hint: crate::similar::suggest(&tool, catalog.functions().map(|f| f.name.as_str())),
        })?;
    if !matches!(
        def.call_kind,
        CallKind::Tool { .. } | CallKind::LlmTable { .. }
    ) {
        return Err(unsupported(
            &format!("CALL {tool}"),
            "CALL is for tools (shell, write_file, patch, web_fetch, ...); use the function in SELECT instead",
        ));
    }
    // `FROM` accepts a table name, an aliased parenthesised subquery, or a
    // bare SELECT/WITH (which gets wrapped and aliased here).
    let from_clause = from.as_ref().map(|q| {
        let head: String = q
            .trim()
            .chars()
            .take(6)
            .collect::<String>()
            .to_ascii_uppercase();
        if head.starts_with("SELECT") || head.starts_with("WITH") {
            format!("({}) AS __call_input", q.trim())
        } else {
            q.trim().to_string()
        }
    });
    let projection = if arg_text.trim().is_empty() {
        "1".to_string()
    } else {
        arg_text.clone()
    };
    let select = match &from_clause {
        Some(f) => format!("SELECT {projection} FROM {f}"),
        None => format!("SELECT {projection}"),
    };
    let stmts = crate::parse(&select)?;
    let planned =
        crate::planner::Planner::new(catalog).plan_statement(&stmts[0], select.clone())?;
    let StatementKind::Query { plan } = planned.kind else {
        return Err(parse_err("CALL arguments must be expressions", HINT));
    };
    let LogicalPlan::Project { input, exprs, .. } = plan else {
        return Err(parse_err("CALL arguments must be expressions", HINT));
    };
    let args = if arg_text.trim().is_empty() {
        vec![]
    } else {
        exprs
    };
    let input = if from.is_some() { Some(*input) } else { None };
    // Arity against the tool's declared signature.
    if args.len() < def.args.len() || (!def.variadic && args.len() > def.args.len()) {
        return Err(SqlError::Type {
            message: format!(
                "{tool} takes {}{} argument{}, got {}",
                if def.variadic { "at least " } else { "" },
                def.args.len(),
                if def.args.len() == 1 { "" } else { "s" },
                args.len()
            ),
        });
    }
    Ok(Statement {
        sql: trimmed.to_string(),
        kind: StatementKind::Call {
            tool: def.name.clone(),
            args,
            input,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_function_forms() {
        let s = plan_create_function(
            "CREATE FUNCTION verify(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is {c} right? It''s important.'",
        )
        .unwrap();
        let StatementKind::CreateFunction {
            name,
            args,
            returns,
            body,
            volatility,
            replace,
            proxy: _,
            model: _,
        } = s.kind
        else {
            panic!()
        };
        assert_eq!(name, "verify");
        assert_eq!(args, vec![("c".to_string(), DataType::Text)]);
        assert_eq!(returns, DataType::Bool);
        assert_eq!(
            body,
            FunctionBody::Prompt {
                template: "Is {c} right? It's important.".into()
            }
        );
        assert_eq!(volatility, Volatility::Immutable);
        assert!(!replace);
        let s = plan_create_function("CREATE OR REPLACE FUNCTION f(x BIGINT, y TEXT) RETURNS TEXT AS SQL (SELECT upper(y)) STABLE;").unwrap();
        assert!(matches!(
            s.kind,
            StatementKind::CreateFunction {
                replace: true,
                body: FunctionBody::Sql { .. },
                volatility: Volatility::Stable,
                ..
            }
        ));
        let s = plan_create_function(
            "CREATE FUNCTION chk(x TEXT) RETURNS BOOLEAN AS SHELL 'python check.py {x}'",
        )
        .unwrap();
        assert!(matches!(
            s.kind,
            StatementKind::CreateFunction {
                volatility: Volatility::Volatile,
                ..
            }
        ));
        let e =
            plan_create_function("CREATE FUNCTION f(x TEXT) RETURNS TEXT AS PROMPT 'hello {y}'")
                .unwrap_err();
        assert!(matches!(e, SqlError::Parse { .. }));
        let e = plan_create_function("CREATE FUNCTION f(x TEXT) RETURNS TEXT AS PYTHON 'x'")
            .unwrap_err();
        assert!(e.hint().is_some());
    }

    #[test]
    fn create_agent_forms() {
        let s = plan_create_agent(
            "CREATE AGENT reviewer MODEL 'worker' EFFORT 'Low' TOOLS (files, grep, Lines) BUDGET (calls 40, tokens 60000, dollars 0.5) PROMPT 'You review; it''s strict.';",
        )
        .unwrap();
        let StatementKind::CreateAgent {
            name,
            model,
            effort,
            tools,
            budget,
            prompt,
            replace,
        } = s.kind
        else {
            panic!()
        };
        assert_eq!(name, "reviewer");
        assert_eq!(model, "worker");
        assert_eq!(effort.as_deref(), Some("low"));
        assert_eq!(tools, ["files", "grep", "lines"]);
        assert_eq!(
            budget,
            vec![
                ("calls".to_string(), 40.0),
                ("tokens".to_string(), 60000.0),
                ("dollars".to_string(), 0.5)
            ]
        );
        assert_eq!(prompt, "You review; it's strict.");
        assert!(!replace);

        let s = plan_create_agent("CREATE OR REPLACE AGENT x").unwrap();
        let StatementKind::CreateAgent {
            model,
            effort,
            tools,
            prompt,
            replace,
            ..
        } = s.kind
        else {
            panic!()
        };
        assert_eq!(model, "worker");
        assert!(effort.is_none() && tools.is_empty() && prompt.is_empty() && replace);

        // Clauses in any order.
        plan_create_agent("CREATE AGENT y PROMPT 'p' MODEL 'root'").unwrap();

        let e = plan_create_agent("CREATE AGENT z COLOUR 'red'").unwrap_err();
        assert!(e.to_string().contains("unexpected clause"), "{e}");
        let e = plan_create_agent("CREATE AGENT z BUDGET (coins 3)").unwrap_err();
        assert!(e.to_string().contains("unknown budget dimension"), "{e}");
        let e = plan_create_agent("CREATE AGENT z EFFORT 'ultra'").unwrap_err();
        assert!(e.to_string().contains("unknown effort"), "{e}");
    }

    #[test]
    fn placeholder_extraction() {
        assert_eq!(
            placeholders("a {x} b {y_2} {} {not a name}"),
            vec!["x", "y_2"]
        );
    }
}
