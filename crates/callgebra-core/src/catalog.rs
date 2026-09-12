//! The catalog: what relations and functions a session can see, and what each
//! one costs to evaluate.
//!
//! The SQL frontend resolves names against it, the planner reads call kinds
//! and volatility from it, the harness renders it into the system prompt, and
//! agent roles are defined as subsets of it.

use crate::value::{DataType, Schema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How often a function's result may change, using PostgreSQL's terms.
///
/// The planner may reorder, deduplicate and memoise `Immutable` calls across
/// runs and `Stable` calls within a statement. `Volatile` operators are
/// fences: evaluated exactly once per input row, in input order, never
/// memoised, never moved past another volatile operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Volatility {
    /// Same inputs, same output, forever.
    Immutable,
    /// Constant within one statement.
    Stable,
    /// Side effects or non-repeatable results.
    Volatile,
}

/// What kind of call evaluating a function implies. This is what turns a
/// relational operator into a call-algebra operator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CallKind {
    /// No external call: ordinary computation.
    Pure,
    /// One model call per distinct argument tuple (`LLM`, `LLM_BOOL`, prompt-defined functions).
    LlmScalar {
        /// Which model tier answers.
        alias: ModelAlias,
    },
    /// One model call per input tuple producing rows (`EXPAND`).
    LlmTable {
        /// Which model tier answers.
        alias: ModelAlias,
    },
    /// A tool invocation (filesystem, shell, web).
    Tool {
        /// Registered tool name.
        tool: String,
    },
    /// A child session (`RLM`, `SPAWN`).
    Recursive {
        /// Agent role that runs the child.
        role: String,
    },
}

/// A model tier name resolved by the router (`root`, `worker`, `proxy`, `judge`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelAlias(pub String);

impl ModelAlias {
    /// The alias for the top-level session.
    pub fn root() -> Self {
        Self("root".into())
    }
    /// The alias for sub-calls and mappers.
    pub fn worker() -> Self {
        Self("worker".into())
    }
    /// The alias for cheap proxy models in cascades.
    pub fn proxy() -> Self {
        Self("proxy".into())
    }
    /// The alias for rubric grading.
    pub fn judge() -> Self {
        Self("judge".into())
    }
}

impl From<&str> for ModelAlias {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Where a relation's rows come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum TableSource {
    /// A table in the session store (created with `CREATE TABLE` or preloaded).
    Stored,
    /// A virtual relation computed on demand (`files`, `INBOX`, `trace_calls`).
    Virtual {
        /// Registered provider name.
        provider: String,
    },
}

/// A relation the model can name in `FROM`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// Name as used in SQL.
    pub name: String,
    /// Columns.
    pub schema: Schema,
    /// Where the rows come from.
    pub source: TableSource,
    /// How stable the contents are within a statement.
    pub volatility: Volatility,
    /// One-line description rendered into the prompt catalog.
    pub description: String,
}

/// What a function returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "returns")]
pub enum FunctionReturn {
    /// A scalar of the given type.
    Scalar {
        /// Result type.
        data_type: DataType,
    },
    /// A relation, usable in `FROM` and `LATERAL`.
    Table {
        /// Result columns.
        schema: Schema,
    },
}

/// A function the model can call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Name as used in SQL (case-insensitive).
    pub name: String,
    /// Argument types in order; `Any` accepts anything.
    pub args: Vec<DataType>,
    /// Whether extra trailing arguments are accepted.
    pub variadic: bool,
    /// Result shape.
    pub returns: FunctionReturn,
    /// What evaluating it costs.
    pub call_kind: CallKind,
    /// How stable the result is.
    pub volatility: Volatility,
    /// One-line description rendered into the prompt catalog.
    pub description: String,
}

/// Everything a session can see.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    tables: BTreeMap<String, TableDef>,
    functions: BTreeMap<String, FunctionDef>,
}

impl Catalog {
    /// An empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace a table.
    pub fn add_table(&mut self, def: TableDef) {
        self.tables.insert(def.name.to_ascii_lowercase(), def);
    }

    /// Register or replace a function.
    pub fn add_function(&mut self, def: FunctionDef) {
        self.functions.insert(def.name.to_ascii_lowercase(), def);
    }

    /// Remove a table; returns the definition if it existed.
    pub fn remove_table(&mut self, name: &str) -> Option<TableDef> {
        self.tables.remove(&name.to_ascii_lowercase())
    }

    /// Remove a function; returns the definition if it existed.
    pub fn remove_function(&mut self, name: &str) -> Option<FunctionDef> {
        self.functions.remove(&name.to_ascii_lowercase())
    }

    /// Look up a table by name (case-insensitive).
    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    /// Look up a function by name (case-insensitive).
    pub fn function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(&name.to_ascii_lowercase())
    }

    /// All tables, sorted by name.
    pub fn tables(&self) -> impl Iterator<Item = &TableDef> {
        self.tables.values()
    }

    /// All functions, sorted by name.
    pub fn functions(&self) -> impl Iterator<Item = &FunctionDef> {
        self.functions.values()
    }

    /// A catalog containing only the named tables and functions: the view an
    /// agent role gets. Unknown names are ignored.
    pub fn subset<'a>(
        &self,
        tables: impl IntoIterator<Item = &'a str>,
        functions: impl IntoIterator<Item = &'a str>,
    ) -> Catalog {
        let mut out = Catalog::new();
        for t in tables {
            if let Some(def) = self.table(t) {
                out.add_table(def.clone());
            }
        }
        for f in functions {
            if let Some(def) = self.function(f) {
                out.add_function(def.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Field;

    fn files() -> TableDef {
        TableDef {
            name: "files".into(),
            schema: Schema::new(vec![Field::new("path", DataType::Text)]),
            source: TableSource::Virtual {
                provider: "fs".into(),
            },
            volatility: Volatility::Stable,
            description: "files in the workspace".into(),
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let mut c = Catalog::new();
        c.add_table(files());
        assert!(c.table("FILES").is_some());
        assert!(c.table("lines").is_none());
    }

    #[test]
    fn subset_keeps_only_named_entries() {
        let mut c = Catalog::new();
        c.add_table(files());
        c.add_function(FunctionDef {
            name: "LLM".into(),
            args: vec![DataType::Text],
            variadic: true,
            returns: FunctionReturn::Scalar {
                data_type: DataType::Text,
            },
            call_kind: CallKind::LlmScalar {
                alias: ModelAlias::worker(),
            },
            volatility: Volatility::Immutable,
            description: "one model call".into(),
        });
        let s = c.subset(["files"], ["nope"]);
        assert!(s.table("files").is_some());
        assert!(s.function("llm").is_none());
    }

    #[test]
    fn volatility_orders_from_safest_to_most_restrictive() {
        assert!(Volatility::Immutable < Volatility::Stable);
        assert!(Volatility::Stable < Volatility::Volatile);
    }
}
