//! Top-level CallSQL statements.

use crate::expr::Expr;
use crate::plan::LogicalPlan;
use callgebra_core::{DataType, Volatility};
use serde::{Deserialize, Serialize};

/// A resolved statement, ready for planning and execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    /// Original text, for the trace.
    pub sql: String,
    /// What it does.
    pub kind: StatementKind,
}

/// The body of a prompt-, SQL- or shell-defined function.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "body")]
pub enum FunctionBody {
    /// `AS PROMPT '...'`: a template with `{arg}` placeholders sent to a model.
    Prompt {
        /// Template text.
        template: String,
    },
    /// `AS SQL (...)`: a query over the arguments.
    Sql {
        /// Query text (planned at call time against the caller's catalog).
        query: String,
    },
    /// `AS SHELL '...'`: a command with `{arg}` placeholders run in the sandbox.
    Shell {
        /// Command template.
        command: String,
    },
}

/// Statement kinds in the CallSQL subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "stmt")]
pub enum StatementKind {
    /// `SELECT ...` (including `WITH`).
    Query {
        /// The plan.
        plan: LogicalPlan,
    },
    /// `CREATE TABLE name AS SELECT ...`.
    CreateTableAs {
        /// New table name.
        name: String,
        /// Source query.
        plan: LogicalPlan,
        /// `IF NOT EXISTS`.
        if_not_exists: bool,
    },
    /// `INSERT INTO name SELECT ...` or `INSERT INTO name VALUES ...`.
    Insert {
        /// Target table.
        table: String,
        /// Source rows.
        plan: LogicalPlan,
    },
    /// `DROP TABLE name`.
    DropTable {
        /// Table name.
        name: String,
        /// `IF EXISTS`.
        if_exists: bool,
    },
    /// `CREATE FUNCTION name(args) RETURNS type AS ...`.
    CreateFunction {
        /// Function name.
        name: String,
        /// `(arg name, type)` pairs.
        args: Vec<(String, DataType)>,
        /// Return type.
        returns: DataType,
        /// Body.
        body: FunctionBody,
        /// Declared volatility; defaults by body kind.
        volatility: Volatility,
        /// `CREATE OR REPLACE`.
        replace: bool,
        /// `PROXY name THRESHOLDS (low, high)`: a cheap scoring function the
        /// planner may cascade this predicate through.
        proxy: Option<(String, f64, f64)>,
        /// `MODEL 'alias'`: the model tier a prompt function runs on
        /// (default: the session's default alias).
        model: Option<String>,
    },
    /// `CREATE AGENT name MODEL '...' EFFORT '...' TOOLS (...) BUDGET (...) PROMPT '...'`.
    CreateAgent {
        /// Role name.
        name: String,
        /// Model alias.
        model: String,
        /// Effort level, if given.
        effort: Option<String>,
        /// Permitted relations and functions.
        tools: Vec<String>,
        /// Budget fields as `(dimension, value)`.
        budget: Vec<(String, f64)>,
        /// System prompt.
        prompt: String,
        /// `OR REPLACE` was given.
        replace: bool,
    },
    /// `CALL tool(args) [FROM query]`: a volatile side effect, once per input row.
    Call {
        /// Tool name.
        tool: String,
        /// Arguments; may reference `input` columns.
        args: Vec<Expr>,
        /// Optional row source.
        input: Option<LogicalPlan>,
    },
    /// `EXPLAIN [ANALYZE] statement`.
    Explain {
        /// The statement to explain.
        inner: Box<Statement>,
        /// Execute and report actuals.
        analyze: bool,
    },
    /// `SET key = value`.
    Set {
        /// Setting name (`budget.calls`, `effort`, `model.default`).
        key: String,
        /// Value as text.
        value: String,
    },
    /// `FINAL(expr)` or `FINAL FROM (query)`: end the session with an answer relation.
    Final {
        /// The answer.
        plan: LogicalPlan,
    },
}
