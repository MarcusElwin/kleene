//! DuckDB-backed [`Store`], trace tables, trace sink and memo.
//!
//! One `duckdb::Connection` is held behind a `std::sync::Mutex` and every
//! operation runs on the blocking thread pool (`spawn_blocking`), because the
//! connection is `Send` but not `Sync`. Rows are written with literal SQL in
//! chunks inside a transaction, which is simple and fast enough for session
//! tables and traces; the appender can replace it later without touching the
//! interface.

use crate::{Store, StoreError};
use callgebra_core::{Batch, DataType, Field, Row, Schema, Value};
use callgebra_trace::{TraceEvent, TraceSink, Traced};
use duckdb::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tables the store keeps for itself; hidden from [`Store::tables`].
const INTERNAL_TABLES: &[&str] = &[
    "trace_runs",
    "trace_sessions",
    "trace_statements",
    "trace_calls",
    "trace_tool_calls",
    "trace_rounds",
    "trace_final",
    "memo",
    "callgebra_columns",
];

/// Embedded DuckDB store.
#[derive(Clone)]
pub struct DuckDbStore {
    conn: Arc<Mutex<Connection>>,
}

fn engine(e: duckdb::Error) -> StoreError {
    let msg = e.to_string();
    if msg.contains("does not exist") {
        // "Catalog Error: Table with name x does not exist!"
        let name = msg
            .split("name ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("?")
            .to_string();
        return StoreError::NoSuchTable(name);
    }
    if msg.contains("already exists") {
        let name = msg.split('"').nth(1).unwrap_or("?").to_string();
        return StoreError::AlreadyExists(name);
    }
    StoreError::Engine(msg)
}

/// Quote an identifier.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Render a value as a SQL literal.
pub fn literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => {
            if f.is_finite() {
                format!("{f:?}")
            } else if f.is_nan() {
                "'NaN'::DOUBLE".into()
            } else if *f > 0.0 {
                "'Infinity'::DOUBLE".into()
            } else {
                "'-Infinity'::DOUBLE".into()
            }
        }
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        // JSON travels as text: DuckDB's JSON type needs the json extension, which
        // the bundled build does not include. The declared type is kept in
        // `callgebra_columns` so scans still yield `Value::Json`.
        Value::Json(j) => format!("'{}'", j.to_string().replace('\'', "''")),
        Value::Vector(v) => format!(
            "[{}]::FLOAT[]",
            v.iter()
                .map(|x| format!("{x:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn sql_type(t: DataType) -> &'static str {
    match t {
        DataType::Any => "VARCHAR",
        DataType::Bool => "BOOLEAN",
        DataType::Int => "BIGINT",
        DataType::Float => "DOUBLE",
        DataType::Text => "VARCHAR",
        DataType::Json => "VARCHAR",
        DataType::Vector => "FLOAT[]",
    }
}

fn data_type_name(t: DataType) -> &'static str {
    match t {
        DataType::Any => "any",
        DataType::Bool => "bool",
        DataType::Int => "int",
        DataType::Float => "float",
        DataType::Text => "text",
        DataType::Json => "json",
        DataType::Vector => "vector",
    }
}

fn data_type_from_name(n: &str) -> DataType {
    match n {
        "bool" => DataType::Bool,
        "int" => DataType::Int,
        "float" => DataType::Float,
        "text" => DataType::Text,
        "json" => DataType::Json,
        "vector" => DataType::Vector,
        _ => DataType::Any,
    }
}

/// Coerce a value read back from DuckDB to its declared CallSQL type.
fn coerce(v: Value, declared: DataType) -> Value {
    match (declared, v) {
        (DataType::Json, Value::Text(s)) => serde_json::from_str(&s)
            .map(Value::Json)
            .unwrap_or(Value::Text(s)),
        (_, v) => v,
    }
}

fn data_type_of(duck: &str) -> DataType {
    let u = duck.to_ascii_uppercase();
    if u == "BOOLEAN" {
        DataType::Bool
    } else if matches!(
        u.as_str(),
        "TINYINT"
            | "SMALLINT"
            | "INTEGER"
            | "BIGINT"
            | "HUGEINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UBIGINT"
    ) {
        DataType::Int
    } else if u == "DOUBLE" || u == "FLOAT" || u == "REAL" || u.starts_with("DECIMAL") {
        DataType::Float
    } else if u == "JSON" {
        DataType::Json
    } else if u.ends_with("[]") {
        DataType::Vector
    } else {
        DataType::Text
    }
}

fn from_duck(v: duckdb::types::Value) -> Result<Value, StoreError> {
    use duckdb::types::Value as D;
    Ok(match v {
        D::Null => Value::Null,
        D::Boolean(b) => Value::Bool(b),
        D::TinyInt(i) => Value::Int(i64::from(i)),
        D::SmallInt(i) => Value::Int(i64::from(i)),
        D::Int(i) => Value::Int(i64::from(i)),
        D::BigInt(i) => Value::Int(i),
        D::UTinyInt(i) => Value::Int(i64::from(i)),
        D::USmallInt(i) => Value::Int(i64::from(i)),
        D::UInt(i) => Value::Int(i64::from(i)),
        D::UBigInt(i) => Value::Int(
            i64::try_from(i)
                .map_err(|_| StoreError::Engine(format!("integer {i} out of range")))?,
        ),
        D::HugeInt(i) => Value::Int(
            i64::try_from(i)
                .map_err(|_| StoreError::Engine(format!("integer {i} out of range")))?,
        ),
        D::UHugeInt(i) => Value::Int(
            i64::try_from(i)
                .map_err(|_| StoreError::Engine(format!("integer {i} out of range")))?,
        ),
        D::Float(f) => Value::Float(f64::from(f)),
        D::Double(f) => Value::Float(f),
        D::Decimal(d) => Value::Float(
            d.to_string()
                .parse()
                .map_err(|_| StoreError::Engine(format!("bad decimal {d}")))?,
        ),
        D::Text(s) => Value::Text(s),
        D::Enum(s) => Value::Text(s),
        D::List(items) | D::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                match from_duck(it)? {
                    Value::Float(f) => out.push(f as f32),
                    Value::Int(i) => out.push(i as f32),
                    Value::Null => out.push(f32::NAN),
                    other => {
                        return Err(StoreError::Engine(format!(
                            "list element {} is not numeric",
                            other.data_type()
                        )))
                    }
                }
            }
            Value::Vector(out)
        }
        D::Timestamp(_, _) | D::Date32(_) | D::Time64(_, _) | D::Interval { .. } => {
            Value::Text(format!("{v:?}"))
        }
        D::Blob(b) | D::Geometry(b) => Value::Text(format!("<{} bytes>", b.len())),
        other => Value::Text(format!("{other:?}")),
    })
}

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

fn ts_literal(t: SystemTime) -> String {
    let micros = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    format!("make_timestamp({micros})")
}

impl DuckDbStore {
    /// Open (or create) a database file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path.as_ref()).map_err(engine)?;
        Self::from_conn(conn)
    }

    /// A private in-memory database.
    pub fn in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(engine)?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self, StoreError> {
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.init_trace_schema_blocking()?;
        Ok(store)
    }

    fn with_conn<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> impl std::future::Future<Output = Result<T, StoreError>> {
        let conn = self.conn.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let guard = conn
                    .lock()
                    .map_err(|_| StoreError::Engine("connection mutex poisoned".into()))?;
                f(&guard)
            })
            .await
            .map_err(|e| StoreError::Engine(format!("store task failed: {e}")))?
        }
    }

    fn query_blocking(conn: &Connection, sql: &str) -> Result<Batch, StoreError> {
        let mut stmt = conn.prepare(sql).map_err(engine)?;
        let mut rows = stmt.query([]).map_err(engine)?;
        // The statement has run at this point, so its column metadata is valid
        // even when no rows come back.
        let names: Vec<String> = match rows.as_ref() {
            Some(st) => (0..st.column_count())
                .map(|i| {
                    st.column_name(i)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| format!("col{i}"))
                })
                .collect(),
            None => vec![],
        };
        let width = names.len();
        let mut out: Vec<Row> = vec![];
        while let Some(row) = rows.next().map_err(engine)? {
            let mut r = Vec::with_capacity(width);
            for i in 0..width {
                let v: duckdb::types::Value = row.get(i).map_err(engine)?;
                r.push(from_duck(v)?);
            }
            out.push(r);
        }
        // Column types: infer from the first non-NULL value; Any when unknown.
        let fields: Vec<Field> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let t = out
                    .iter()
                    .map(|r| r[i].data_type())
                    .find(|t| *t != DataType::Any)
                    .unwrap_or(DataType::Any);
                Field::new(n.clone(), t)
            })
            .collect();
        Batch::try_new(Arc::new(Schema::new(fields)), out)
            .map_err(|e| StoreError::Engine(e.to_string()))
    }

    /// Run arbitrary SQL and return its rows.
    pub async fn query(&self, sql: &str) -> Result<Batch, StoreError> {
        let sql = sql.to_string();
        self.with_conn(move |c| Self::query_blocking(c, &sql)).await
    }

    /// Run arbitrary SQL that returns no rows; returns rows affected.
    pub async fn execute(&self, sql: &str) -> Result<usize, StoreError> {
        let sql = sql.to_string();
        self.with_conn(move |c| c.execute(&sql, []).map_err(engine))
            .await
    }

    fn init_trace_schema_blocking(&self) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Engine("connection mutex poisoned".into()))?;
        conn.execute_batch(TRACE_DDL).map_err(engine)
    }

    /// Create the trace and memo tables if they do not exist. Called on open.
    pub async fn init_trace_schema(&self) -> Result<(), StoreError> {
        self.with_conn(|c| c.execute_batch(TRACE_DDL).map_err(engine))
            .await
    }

    /// A trace sink writing into this store.
    pub fn trace_sink(&self) -> DuckDbTraceSink {
        DuckDbTraceSink::new(self.clone())
    }

    /// Look a memoised response up.
    pub async fn memo_get(
        &self,
        model: &str,
        fingerprint: &str,
    ) -> Result<Option<MemoEntry>, StoreError> {
        let sql = format!(
            "SELECT response, input_tokens, output_tokens, cost_usd FROM memo WHERE model = {} AND fingerprint = {}",
            literal(&Value::Text(model.into())),
            literal(&Value::Text(fingerprint.into()))
        );
        let b = self.query(&sql).await?;
        Ok(b.rows.into_iter().next().map(|r| MemoEntry {
            response: match &r[0] {
                Value::Json(j) => j.clone(),
                Value::Text(s) => {
                    serde_json::from_str(s).unwrap_or(serde_json::Value::String(s.clone()))
                }
                other => serde_json::Value::String(other.render()),
            },
            input_tokens: r[1].as_int().unwrap_or(0) as u64,
            output_tokens: r[2].as_int().unwrap_or(0) as u64,
            cost_usd: r[3].as_f64(),
        }))
    }

    /// Store a memoised response (replacing any previous one).
    pub async fn memo_put(
        &self,
        model: &str,
        fingerprint: &str,
        entry: &MemoEntry,
    ) -> Result<(), StoreError> {
        let sql = format!(
            "INSERT OR REPLACE INTO memo (model, fingerprint, response, input_tokens, output_tokens, cost_usd, created_at) VALUES ({}, {}, {}, {}, {}, {}, make_timestamp({}))",
            literal(&Value::Text(model.into())),
            literal(&Value::Text(fingerprint.into())),
            literal(&Value::Json(entry.response.clone())),
            entry.input_tokens,
            entry.output_tokens,
            entry.cost_usd.map(|c| c.to_string()).unwrap_or_else(|| "NULL".into()),
            now_micros()
        );
        self.execute(&sql).await.map(|_| ())
    }
}

/// A memoised model response.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoEntry {
    /// The serialised response.
    pub response: serde_json::Value,
    /// Input tokens of the original call.
    pub input_tokens: u64,
    /// Output tokens of the original call.
    pub output_tokens: u64,
    /// Cost of the original call, if known.
    pub cost_usd: Option<f64>,
}

const TRACE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS trace_runs (run VARCHAR PRIMARY KEY, started_at TIMESTAMP, task VARCHAR);
CREATE TABLE IF NOT EXISTS trace_sessions (session VARCHAR PRIMARY KEY, run VARCHAR, parent VARCHAR, depth INTEGER, role VARCHAR, started_at TIMESTAMP, task VARCHAR, finished_at TIMESTAMP, outcome VARCHAR, turns INTEGER, calls BIGINT, tokens BIGINT, dollars DOUBLE);
CREATE TABLE IF NOT EXISTS trace_statements (statement VARCHAR PRIMARY KEY, session VARCHAR, sql VARCHAR, started_at TIMESTAMP, finished_at TIMESTAMP, rows BIGINT, calls BIGINT, tokens BIGINT, dollars DOUBLE, error VARCHAR, explain VARCHAR, estimate VARCHAR);
CREATE TABLE IF NOT EXISTS trace_calls (call VARCHAR PRIMARY KEY, statement VARCHAR, alias VARCHAR, model VARCHAR, fingerprint VARCHAR, started_at TIMESTAMP, finished_at TIMESTAMP, input_tokens BIGINT, output_tokens BIGINT, cache_read_tokens BIGINT, cost_usd DOUBLE, memo_hit BOOLEAN, error VARCHAR);
CREATE TABLE IF NOT EXISTS trace_tool_calls (statement VARCHAR, tool VARCHAR, args VARCHAR, bytes_out BIGINT, elapsed_ms BIGINT, error VARCHAR, recorded_at TIMESTAMP);
CREATE TABLE IF NOT EXISTS trace_rounds (statement VARCHAR, cte VARCHAR, round INTEGER, delta_rows BIGINT, total_rows BIGINT, recorded_at TIMESTAMP);
CREATE TABLE IF NOT EXISTS trace_final (session VARCHAR, rows BIGINT, recorded_at TIMESTAMP);
CREATE TABLE IF NOT EXISTS memo (model VARCHAR, fingerprint VARCHAR, response VARCHAR, input_tokens BIGINT, output_tokens BIGINT, cost_usd DOUBLE, created_at TIMESTAMP, PRIMARY KEY (model, fingerprint));
CREATE TABLE IF NOT EXISTS callgebra_columns (table_name VARCHAR, column_index INTEGER, column_name VARCHAR, data_type VARCHAR);
"#;

#[async_trait::async_trait]
impl Store for DuckDbStore {
    async fn create_table(&self, name: &str, schema: Arc<Schema>) -> Result<(), StoreError> {
        let cols: Vec<String> = schema
            .fields
            .iter()
            .map(|f| format!("{} {}", quote_ident(&f.name), sql_type(f.data_type)))
            .collect();
        let cols = if cols.is_empty() {
            "\"__empty\" BOOLEAN".to_string()
        } else {
            cols.join(", ")
        };
        let mut sql = format!("CREATE TABLE {} ({});", quote_ident(name), cols);
        for (i, f) in schema.fields.iter().enumerate() {
            sql += &format!(
                "INSERT INTO callgebra_columns VALUES ({}, {i}, {}, '{}');",
                literal(&Value::Text(name.into())),
                literal(&Value::Text(f.name.clone())),
                data_type_name(f.data_type)
            );
        }
        let name = name.to_string();
        self.with_conn(move |c| {
            c.execute_batch(&sql).map_err(|e| match engine(e) {
                StoreError::AlreadyExists(_) => StoreError::AlreadyExists(name.clone()),
                other => other,
            })
        })
        .await
    }

    async fn insert(&self, name: &str, batch: Batch) -> Result<(), StoreError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let name = name.to_string();
        self.with_conn(move |c| {
            let table = quote_ident(&name);
            c.execute_batch("BEGIN").map_err(engine)?;
            let result = (|| {
                for chunk in batch.rows.chunks(500) {
                    let values: Vec<String> = chunk
                        .iter()
                        .map(|r| {
                            format!("({})", r.iter().map(literal).collect::<Vec<_>>().join(", "))
                        })
                        .collect();
                    let sql = format!("INSERT INTO {table} VALUES {}", values.join(", "));
                    c.execute_batch(&sql).map_err(|e| match engine(e) {
                        StoreError::NoSuchTable(_) => StoreError::NoSuchTable(name.clone()),
                        StoreError::Engine(m)
                            if m.contains("Binder Error") || m.contains("Conversion Error") =>
                        {
                            StoreError::SchemaMismatch {
                                table: name.clone(),
                                detail: m,
                            }
                        }
                        other => other,
                    })?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => c.execute_batch("COMMIT").map_err(engine),
                Err(e) => {
                    let _ = c.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
        .await
    }

    async fn drop_table(&self, name: &str) -> Result<(), StoreError> {
        let sql = format!(
            "DROP TABLE {}; DELETE FROM callgebra_columns WHERE table_name = {};",
            quote_ident(name),
            literal(&Value::Text(name.into()))
        );
        let name = name.to_string();
        self.with_conn(move |c| {
            c.execute_batch(&sql).map_err(|e| match engine(e) {
                StoreError::NoSuchTable(_) => StoreError::NoSuchTable(name.clone()),
                other => other,
            })
        })
        .await
    }

    async fn scan(&self, name: &str) -> Result<Batch, StoreError> {
        let Some(schema) = self.schema(name).await? else {
            return Err(StoreError::NoSuchTable(name.to_string()));
        };
        let sql = format!("SELECT * FROM {}", quote_ident(name));
        let b = self.query(&sql).await?;
        // Use the declared schema (query() infers types from values, which loses
        // them on all-NULL columns) and coerce text back to JSON where declared.
        let rows: Vec<Row> = b
            .rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .zip(&schema.fields)
                    .map(|(v, f)| coerce(v, f.data_type))
                    .collect()
            })
            .collect();
        Batch::try_new(schema, rows).map_err(|e| StoreError::Engine(e.to_string()))
    }

    async fn schema(&self, name: &str) -> Result<Option<Arc<Schema>>, StoreError> {
        // Declared CallSQL types first (created through this store).
        let declared = self
            .query(&format!(
                "SELECT column_name, data_type FROM callgebra_columns WHERE table_name = {} ORDER BY column_index",
                literal(&Value::Text(name.into()))
            ))
            .await?;
        if !declared.rows.is_empty() {
            let exists = self
                .query(&format!(
                    "SELECT 1 FROM duckdb_tables() WHERE table_name = {}",
                    literal(&Value::Text(name.into()))
                ))
                .await?;
            if exists.rows.is_empty() {
                return Ok(None);
            }
            return Ok(Some(Arc::new(Schema::new(
                declared
                    .rows
                    .iter()
                    .map(|r| Field::new(r[0].render(), data_type_from_name(&r[1].render())))
                    .collect(),
            ))));
        }
        let sql = format!(
            "SELECT column_name, data_type FROM duckdb_columns() WHERE table_name = {} ORDER BY column_index",
            literal(&Value::Text(name.into()))
        );
        let b = self.query(&sql).await?;
        if b.rows.is_empty() {
            return Ok(None);
        }
        let fields = b
            .rows
            .iter()
            .map(|r| Field::new(r[0].render(), data_type_of(&r[1].render())))
            .filter(|f| f.name != "__empty")
            .collect();
        Ok(Some(Arc::new(Schema::new(fields))))
    }

    async fn tables(&self) -> Result<Vec<String>, StoreError> {
        let b = self
            .query("SELECT table_name FROM duckdb_tables() WHERE NOT internal ORDER BY table_name")
            .await?;
        Ok(b.rows
            .into_iter()
            .map(|r| r[0].render())
            .filter(|n| !INTERNAL_TABLES.contains(&n.as_str()))
            .collect())
    }
}

/// Writes trace events into the store's trace tables from a background task.
/// Clones share the writer; [`DuckDbTraceSink::flush`] on any clone waits for
/// everything recorded so far.
#[derive(Clone)]
pub struct DuckDbTraceSink {
    tx: tokio::sync::mpsc::UnboundedSender<Msg>,
}

enum Msg {
    Event(Traced),
    Flush(tokio::sync::oneshot::Sender<()>),
}

impl DuckDbTraceSink {
    /// Start the writer task over `store`. Must be called inside a Tokio runtime.
    pub fn new(store: DuckDbStore) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    Msg::Event(t) => {
                        let sql = event_sql(&t);
                        if let Err(e) = store.execute(&sql).await {
                            tracing::warn!(target: "callgebra::trace", "trace write failed: {e}; sql: {sql}");
                        }
                    }
                    Msg::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        Self { tx }
    }

    /// Wait until every event recorded so far has been written.
    pub async fn flush(&self) {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        if self.tx.send(Msg::Flush(done_tx)).is_ok() {
            let _ = done_rx.await;
        }
    }
}

impl TraceSink for DuckDbTraceSink {
    fn record(&self, event: Traced) {
        let _ = self.tx.send(Msg::Event(event));
    }
}

fn s(x: &str) -> String {
    literal(&Value::Text(x.into()))
}

fn opt(x: &Option<String>) -> String {
    match x {
        Some(v) => s(v),
        None => "NULL".into(),
    }
}

/// The SQL that records one event.
fn event_sql(t: &Traced) -> String {
    let at = ts_literal(t.at);
    match &t.event {
        TraceEvent::RunStarted { run, task } => format!(
            "INSERT OR REPLACE INTO trace_runs VALUES ({}, {at}, {})",
            s(&run.to_string()),
            s(task)
        ),
        TraceEvent::SessionStarted { run, session, parent, depth, role, task } => format!(
            "INSERT OR REPLACE INTO trace_sessions (session, run, parent, depth, role, started_at, task) VALUES ({}, {}, {}, {depth}, {}, {at}, {})",
            s(&session.to_string()),
            s(&run.to_string()),
            opt(&parent.map(|p| p.to_string())),
            s(role),
            s(task)
        ),
        TraceEvent::SessionFinished { session, outcome, turns, usage } => format!(
            "UPDATE trace_sessions SET finished_at = {at}, outcome = {}, turns = {turns}, calls = {}, tokens = {}, dollars = {} WHERE session = {}",
            s(outcome),
            usage.calls,
            usage.tokens,
            usage.dollars,
            s(&session.to_string())
        ),
        TraceEvent::StatementStarted { session, statement, sql } => format!(
            "INSERT OR REPLACE INTO trace_statements (statement, session, sql, started_at) VALUES ({}, {}, {}, {at})",
            s(&statement.to_string()),
            s(&session.to_string()),
            s(sql)
        ),
        TraceEvent::StatementPlanned { statement, explain, estimate } => format!(
            "UPDATE trace_statements SET explain = {}, estimate = {} WHERE statement = {}",
            s(explain),
            literal(&Value::Json(estimate.clone())),
            s(&statement.to_string())
        ),
        TraceEvent::StatementFinished { statement, rows, usage, error, .. } => format!(
            "UPDATE trace_statements SET finished_at = {at}, rows = {rows}, calls = {}, tokens = {}, dollars = {}, error = {} WHERE statement = {}",
            usage.calls,
            usage.tokens,
            usage.dollars,
            opt(error),
            s(&statement.to_string())
        ),
        TraceEvent::CallStarted { statement, call, alias, model, fingerprint } => format!(
            "INSERT OR REPLACE INTO trace_calls (call, statement, alias, model, fingerprint, started_at) VALUES ({}, {}, {}, {}, {}, {at})",
            s(&call.to_string()),
            s(&statement.to_string()),
            s(alias),
            s(model),
            s(fingerprint)
        ),
        TraceEvent::CallFinished { call, input_tokens, output_tokens, cache_read_tokens, cost_usd, memo_hit, error, .. } => format!(
            "UPDATE trace_calls SET finished_at = {at}, input_tokens = {input_tokens}, output_tokens = {output_tokens}, cache_read_tokens = {cache_read_tokens}, cost_usd = {cost_usd}, memo_hit = {memo_hit}, error = {} WHERE call = {}",
            opt(error),
            s(&call.to_string())
        ),
        TraceEvent::ToolCall { statement, tool, args, bytes_out, elapsed, error } => format!(
            "INSERT INTO trace_tool_calls VALUES ({}, {}, {}, {bytes_out}, {}, {}, {at})",
            s(&statement.to_string()),
            s(tool),
            s(args),
            elapsed.as_millis(),
            opt(error)
        ),
        TraceEvent::RecursionRound { statement, cte, round, delta_rows, total_rows } => format!(
            "INSERT INTO trace_rounds VALUES ({}, {}, {round}, {delta_rows}, {total_rows}, {at})",
            s(&statement.to_string()),
            s(cte)
        ),
        TraceEvent::BudgetExceeded { statement, detail } => format!(
            "UPDATE trace_statements SET error = {} WHERE statement = {}",
            s(&format!("budget exceeded: {detail}")),
            s(&statement.to_string())
        ),
        TraceEvent::Final { session, rows } => format!(
            "INSERT INTO trace_final VALUES ({}, {rows}, {at})",
            s(&session.to_string())
        ),
    }
}
