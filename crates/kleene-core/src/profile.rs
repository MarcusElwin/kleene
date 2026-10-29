//! Capability profiles: which tools a root session, and every child under
//! it, may use.
//!
//! A profile narrows the tool catalog the same way a `CREATE AGENT ...
//! TOOLS (...)` role narrows a child's: the withheld tools are removed from
//! the catalog, so the SQL frontend does not resolve them and the prompt
//! does not list them. The three flags are shortcuts over one mechanism, an
//! allow-list, and they compose: `--read-only --tools (files, grep, shell)`
//! leaves `files` and `grep`.

use crate::{CallKind, Catalog, FunctionDef, Volatility};
use serde::{Deserialize, Serialize};

/// The tools `--no-web` withholds.
pub const WEB_TOOLS: [&str; 2] = ["web_fetch", "web_search"];

/// The tool `--no-shell` withholds (and the tool a `CREATE FUNCTION ... AS
/// SHELL` body needs).
pub const SHELL_TOOL: &str = "shell";

/// Which tools a session may use. The default allows everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProfile {
    /// No `VOLATILE` tool at all: nothing that changes the workspace,
    /// reaches the network or messages another session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// No `shell`, and no `CREATE FUNCTION ... AS SHELL`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_shell: bool,
    /// No `web_fetch` or `web_search`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_web: bool,
    /// Only these tools, by name (case does not matter); `None` leaves every
    /// tool the flags allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

impl ToolProfile {
    /// Every tool allowed.
    pub fn is_unrestricted(&self) -> bool {
        *self == Self::default()
    }

    /// Whether a catalog function passes the profile. Only tools are judged;
    /// model calls, delegation and pure functions always pass.
    pub fn allows(&self, def: &FunctionDef) -> bool {
        let CallKind::Tool { tool } = &def.call_kind else {
            return true;
        };
        let name = def.name.to_ascii_lowercase();
        let tool = tool.to_ascii_lowercase();
        if self.read_only && def.volatility == Volatility::Volatile {
            return false;
        }
        if self.no_shell && (name == SHELL_TOOL || tool == SHELL_TOOL) {
            return false;
        }
        if self.no_web && WEB_TOOLS.contains(&name.as_str()) {
            return false;
        }
        match &self.tools {
            Some(list) => list.iter().any(|t| t.eq_ignore_ascii_case(&name)),
            None => true,
        }
    }

    /// Remove every tool the profile withholds from a catalog and return
    /// their names, sorted.
    pub fn apply(&self, catalog: &mut Catalog) -> Vec<String> {
        let mut denied: Vec<String> = catalog
            .functions()
            .filter(|f| !self.allows(f))
            .map(|f| f.name.clone())
            .collect();
        denied.sort();
        for name in &denied {
            catalog.remove_function(name);
        }
        denied
    }

    /// The profile as the flags that produce it, for a prompt or a log line:
    /// `--read-only`, `--no-shell --no-web`, `--tools files,grep`; empty when
    /// unrestricted.
    pub fn describe(&self) -> String {
        let mut parts = vec![];
        if self.read_only {
            parts.push("--read-only".to_string());
        }
        if self.no_shell {
            parts.push("--no-shell".to_string());
        }
        if self.no_web {
            parts.push("--no-web".to_string());
        }
        if let Some(list) = &self.tools {
            parts.push(format!("--tools {}", list.join(",")));
        }
        parts.join(" ")
    }

    /// This profile narrowed further by another: a flag set in either is
    /// set, and an allow-list in both keeps only the names in both.
    pub fn intersect(&self, other: &Self) -> Self {
        let tools = match (&self.tools, &other.tools) {
            (Some(a), Some(b)) => Some(
                a.iter()
                    .filter(|t| b.iter().any(|u| u.eq_ignore_ascii_case(t)))
                    .cloned()
                    .collect(),
            ),
            (Some(a), None) | (None, Some(a)) => Some(a.clone()),
            (None, None) => None,
        };
        Self {
            read_only: self.read_only || other.read_only,
            no_shell: self.no_shell || other.no_shell,
            no_web: self.no_web || other.no_web,
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataType, Field, FunctionReturn, Schema};

    fn tool(name: &str, volatility: Volatility) -> FunctionDef {
        FunctionDef {
            name: name.into(),
            args: vec![],
            variadic: false,
            returns: FunctionReturn::Table {
                schema: Schema::new(vec![Field::not_null("x", DataType::Text)]),
            },
            call_kind: CallKind::Tool { tool: name.into() },
            volatility,
            description: String::new(),
        }
    }

    fn catalog() -> Catalog {
        let mut c = crate::standard_catalog();
        for (n, v) in [
            ("files", Volatility::Stable),
            ("grep", Volatility::Stable),
            ("shell", Volatility::Volatile),
            ("write_file", Volatility::Volatile),
            ("web_fetch", Volatility::Volatile),
            ("web_search", Volatility::Volatile),
            ("mcp_db_insert", Volatility::Volatile),
            ("mcp_db_query", Volatility::Stable),
        ] {
            c.add_function(tool(n, v));
        }
        c
    }

    #[test]
    fn the_default_withholds_nothing() {
        let p = ToolProfile::default();
        assert!(p.is_unrestricted());
        assert_eq!(p.describe(), "");
        let mut c = catalog();
        assert!(p.apply(&mut c).is_empty());
    }

    #[test]
    fn read_only_withholds_every_volatile_tool_including_send_and_mcp() {
        let p = ToolProfile {
            read_only: true,
            ..Default::default()
        };
        let mut c = catalog();
        assert_eq!(
            p.apply(&mut c),
            vec![
                "mcp_db_insert",
                "send",
                "shell",
                "web_fetch",
                "web_search",
                "write_file"
            ]
        );
        assert!(c.function("grep").is_some());
        assert!(c.function("mcp_db_query").is_some());
        assert!(c.function("inbox").is_some());
        assert!(c.function("llm").is_some(), "model calls are not tools");
        assert!(c.function("rlm").is_some(), "delegation is not a tool");
        assert_eq!(p.describe(), "--read-only");
    }

    #[test]
    fn no_shell_and_no_web_withhold_their_tools_only() {
        let p = ToolProfile {
            no_shell: true,
            no_web: true,
            ..Default::default()
        };
        let mut c = catalog();
        assert_eq!(p.apply(&mut c), vec!["shell", "web_fetch", "web_search"]);
        assert!(c.function("write_file").is_some());
        assert_eq!(p.describe(), "--no-shell --no-web");
        // A shell-bodied function is a tool call on `shell` under another name.
        let mut f = tool("run_tests", Volatility::Volatile);
        f.call_kind = CallKind::Tool {
            tool: "shell".into(),
        };
        assert!(!p.allows(&f));
    }

    #[test]
    fn an_allow_list_keeps_only_its_names_and_composes_with_the_flags() {
        let p = ToolProfile {
            read_only: true,
            tools: Some(vec!["Files".into(), "shell".into()]),
            ..Default::default()
        };
        let mut c = catalog();
        let denied = p.apply(&mut c);
        assert!(denied.contains(&"grep".to_string()));
        assert!(denied.contains(&"shell".to_string()), "read-only wins");
        assert!(c.function("files").is_some());
        assert_eq!(p.describe(), "--read-only --tools Files,shell");
    }

    #[test]
    fn intersect_only_narrows() {
        let a = ToolProfile {
            no_web: true,
            tools: Some(vec!["files".into(), "grep".into()]),
            ..Default::default()
        };
        let b = ToolProfile {
            no_shell: true,
            tools: Some(vec!["GREP".into(), "shell".into()]),
            ..Default::default()
        };
        let both = a.intersect(&b);
        assert!(both.no_web && both.no_shell && !both.read_only);
        assert_eq!(both.tools, Some(vec!["grep".to_string()]));
        assert_eq!(a.intersect(&ToolProfile::default()), a);
    }

    #[test]
    fn serde_omits_what_is_off() {
        let p = ToolProfile {
            no_web: true,
            ..Default::default()
        };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"no_web":true}"#);
        let back: ToolProfile = serde_json::from_str("{}").unwrap();
        assert!(back.is_unrestricted());
    }
}
