//! Top-level CallSQL statements.

use crate::expr::Expr;
use crate::plan::LogicalPlan;
use kleene_core::{DataType, Volatility};
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
        /// `BATCH n`: the function may answer up to `n` argument tuples in
        /// one call; the executor folds distinct tuples into one prompt that
        /// asks for a JSON array of answers.
        #[serde(default)]
        batch: Option<usize>,
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

/// `PROXY name [THRESHOLDS (low, high)]` on a `CREATE FUNCTION`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyClause {
    /// The scoring function (same arguments as the oracle, returns `DOUBLE`).
    pub function: String,
    /// `(low, high)` when declared; `None` leaves them to a calibration or
    /// the defaults `(0.2, 0.8)`.
    pub thresholds: Option<(f64, f64)>,
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
        /// `PROXY name [THRESHOLDS (low, high)]`: a cheap scoring function
        /// the planner may cascade this predicate through.
        proxy: Option<ProxyClause>,
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
    /// `CALIBRATE name [SAMPLE n] [RECALL r] [PRECISION p] FROM query`: set
    /// the cascade thresholds of a predicate's proxy from a sample of rows
    /// scored by both.
    Calibrate {
        /// The oracle predicate whose proxy is calibrated.
        function: String,
        /// Rows sampled from the query (its first `sample` rows).
        sample: usize,
        /// The share of oracle-true rows the band must keep (`low`).
        recall: f64,
        /// The share of proxy-accepted rows that must be oracle-true (`high`).
        precision: f64,
        /// The rows to sample: the function's arguments, in order.
        input: LogicalPlan,
    },
}
