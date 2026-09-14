//! `LATERAL` table calls run one call per input row, concurrently for call
//! functions, with output in input order.

use kleene_core::{
    standard_catalog, Batch, CallKind, DataType, Field, FunctionDef, FunctionReturn, Schema,
    TableDef, TableSource, Value, Volatility,
};
use kleene_exec::{
    execute_statement, BatchStream, CallSink, ExecContext, ExecError, StatementResult,
};
use kleene_sql::plan_sql;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct CountingSink {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

#[async_trait::async_trait]
impl CallSink for CountingSink {
    async fn scalar_call(&self, name: &str, _args: &[Value]) -> Result<Value, ExecError> {
        Err(ExecError::Call(name.into()))
    }

    async fn table_call(&self, _name: &str, args: &[Value]) -> Result<Batch, ExecError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let n = args[0].as_int().unwrap_or(0);
        let schema = Arc::new(Schema::new(vec![Field::new("echo", DataType::Text)]));
        Batch::try_new(schema, vec![vec![Value::Text(format!("row {n}"))]]).map_err(Into::into)
    }

    async fn scan(&self, _table: &str) -> Result<BatchStream, ExecError> {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int)]));
        let rows = (1..=8).map(|i| vec![Value::Int(i)]).collect();
        Ok(kleene_exec::chunk(schema, rows, 1024))
    }

    async fn create_table(
        &self,
        _name: &str,
        _schema: Arc<Schema>,
        _if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        unreachable!()
    }

    async fn insert(&self, _name: &str, _batch: Batch) -> Result<(), ExecError> {
        unreachable!()
    }

    async fn drop_table(&self, _name: &str, _if_exists: bool) -> Result<(), ExecError> {
        unreachable!()
    }
}

async fn run(concurrency: usize) -> (Vec<Vec<Value>>, usize) {
    let mut catalog = standard_catalog();
    catalog.add_table(TableDef {
        name: "t".into(),
        schema: Schema::new(vec![Field::new("n", DataType::Int)]),
        source: TableSource::Stored,
        volatility: Volatility::Stable,
        description: String::new(),
    });
    catalog.add_function(FunctionDef {
        name: "echo".into(),
        args: vec![DataType::Int],
        variadic: false,
        returns: FunctionReturn::Table {
            schema: Schema::new(vec![Field::new("echo", DataType::Text)]),
        },
        call_kind: CallKind::Tool {
            tool: "echo".into(),
        },
        volatility: Volatility::Stable,
        description: String::new(),
    });
    let sink = Arc::new(CountingSink {
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    let mut ctx = ExecContext::new(sink.clone());
    ctx.call_concurrency = concurrency;
    let stmts = plan_sql(
        "SELECT t.n, e.echo FROM t CROSS JOIN LATERAL echo(t.n) AS e",
        &catalog,
    )
    .unwrap();
    let StatementResult::Rows(b) = execute_statement(&stmts[0], ctx).await.unwrap() else {
        panic!()
    };
    (b.rows, sink.max_in_flight.load(Ordering::SeqCst))
}

#[tokio::test]
async fn lateral_calls_run_concurrently_in_input_order() {
    let (rows, max) = run(4).await;
    assert!(max > 1, "max in flight {max}");
    assert_eq!(rows.len(), 8);
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r[0], Value::Int(i as i64 + 1));
        assert_eq!(r[1], Value::Text(format!("row {}", i + 1)));
    }
}

#[tokio::test]
async fn lateral_calls_are_sequential_at_concurrency_one() {
    let (rows, max) = run(1).await;
    assert_eq!(max, 1);
    assert_eq!(rows.len(), 8);
}
