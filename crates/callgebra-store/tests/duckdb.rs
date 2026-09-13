//! DuckDB store tests: round trips, arbitrary SQL, trace sink, memo, persistence.

use callgebra_core::{
    BudgetUsage, CallId, DataType, Field, RunId, Schema, SessionId, StatementId, Value,
};
use callgebra_store::{DuckDbStore, MemoEntry, Store, StoreError};
use callgebra_trace::{TraceEvent, TraceSink, Tracer};
use std::sync::Arc;
use std::time::Duration;

fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("b", DataType::Bool),
        Field::new("i", DataType::Int),
        Field::new("f", DataType::Float),
        Field::new("t", DataType::Text),
        Field::new("j", DataType::Json),
        Field::new("v", DataType::Vector),
    ]))
}

fn rows() -> Vec<Vec<Value>> {
    vec![
        vec![
            Value::Bool(true),
            Value::Int(-7),
            Value::Float(2.5),
            Value::from("it's \"quoted\""),
            Value::Json(serde_json::json!({"a": [1, 2, {"b": null}]})),
            Value::Vector(vec![0.5, 1.5]),
        ],
        vec![Value::Null; 6],
    ]
}

#[tokio::test]
async fn round_trip_every_type_including_nulls() {
    let s = DuckDbStore::in_memory().unwrap();
    s.create_table("T", schema()).await.unwrap();
    let b = callgebra_core::Batch::try_new(schema(), rows()).unwrap();
    s.insert("T", b).await.unwrap();
    let back = s.scan("T").await.unwrap();
    assert_eq!(back.schema.names(), ["b", "i", "f", "t", "j", "v"]);
    assert_eq!(back.rows, rows());
    let sch = s.schema("T").await.unwrap().unwrap();
    assert_eq!(sch.fields[4].data_type, DataType::Json);
    assert_eq!(sch.fields[5].data_type, DataType::Vector);
    assert!(s.schema("nope").await.unwrap().is_none());
}

#[tokio::test]
async fn tables_hide_internal_ones_and_drop_works() {
    let s = DuckDbStore::in_memory().unwrap();
    assert!(s.tables().await.unwrap().is_empty());
    s.create_table("a", schema()).await.unwrap();
    s.create_table("b", schema()).await.unwrap();
    assert_eq!(
        s.tables().await.unwrap(),
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(matches!(
        s.create_table("a", schema()).await,
        Err(StoreError::AlreadyExists(_))
    ));
    s.drop_table("a").await.unwrap();
    assert_eq!(s.tables().await.unwrap(), vec!["b".to_string()]);
    assert!(matches!(
        s.drop_table("a").await,
        Err(StoreError::NoSuchTable(_))
    ));
    assert!(matches!(s.scan("a").await, Err(StoreError::NoSuchTable(_))));
    let empty = Arc::new(Schema::empty());
    s.create_table("e", empty).await.unwrap();
    assert!(s.schema("e").await.unwrap().unwrap().is_empty());
}

#[tokio::test]
async fn arbitrary_sql_query_and_execute() {
    let s = DuckDbStore::in_memory().unwrap();
    let b = s
        .query("SELECT 1 AS x, 'a' AS y, 2.5 AS z, NULL AS n")
        .await
        .unwrap();
    assert_eq!(b.schema.names(), ["x", "y", "z", "n"]);
    assert_eq!(
        b.rows,
        vec![vec![
            Value::Int(1),
            Value::from("a"),
            Value::Float(2.5),
            Value::Null
        ]]
    );
    let b = s
        .query("WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 4) SELECT SUM(n) FROM r")
        .await
        .unwrap();
    assert_eq!(b.rows, vec![vec![Value::Int(10)]]);
    let b = s.query("SELECT 1 AS x WHERE false").await.unwrap();
    assert!(b.rows.is_empty());
    assert_eq!(b.schema.names(), ["x"]);
    s.execute("CREATE TABLE k (a INTEGER)").await.unwrap();
    let n = s.execute("INSERT INTO k VALUES (1), (2)").await.unwrap();
    assert_eq!(n, 2);
    assert!(s.query("SELECT * FROM nope").await.is_err());
}

#[tokio::test]
async fn trace_sink_records_every_event_kind() {
    let s = DuckDbStore::in_memory().unwrap();
    let sink = Arc::new(s.trace_sink());
    let tracer = Tracer::new(sink.clone());
    let run = RunId::new();
    let session = SessionId::new();
    let statement = StatementId::new();
    let call = CallId::new();
    tracer.emit(TraceEvent::RunStarted {
        run,
        task: "t".into(),
    });
    tracer.emit(TraceEvent::SessionStarted {
        run,
        session,
        parent: None,
        depth: 0,
        role: "self".into(),
    });
    tracer.emit(TraceEvent::StatementStarted {
        session,
        statement,
        sql: "SELECT 'x'".into(),
    });
    tracer.emit(TraceEvent::StatementPlanned {
        statement,
        explain: "π".into(),
        estimate: serde_json::json!({"calls": 1}),
    });
    tracer.emit(TraceEvent::CallStarted {
        statement,
        call,
        alias: "worker".into(),
        model: "m".into(),
        fingerprint: "fp".into(),
    });
    tracer.emit(TraceEvent::CallFinished {
        call,
        input_tokens: 10,
        output_tokens: 5,
        cache_read_tokens: 0,
        cost_usd: 0.001,
        elapsed: Duration::from_millis(3),
        memo_hit: false,
        error: None,
    });
    tracer.emit(TraceEvent::ToolCall {
        statement,
        tool: "grep".into(),
        args: "x".into(),
        bytes_out: 12,
        elapsed: Duration::from_millis(1),
        error: None,
    });
    tracer.emit(TraceEvent::RecursionRound {
        statement,
        cte: "reach".into(),
        round: 1,
        delta_rows: 3,
        total_rows: 5,
    });
    tracer.emit(TraceEvent::StatementFinished {
        statement,
        rows: 1,
        usage: BudgetUsage {
            calls: 1,
            tokens: 15,
            dollars: 0.001,
            wall: Duration::from_millis(4),
        },
        error: None,
        elapsed: Duration::from_millis(4),
    });
    tracer.emit(TraceEvent::Final { session, rows: 1 });
    sink.flush().await;
    let b = s
        .query("SELECT sql, rows, calls, tokens, explain, error FROM trace_statements")
        .await
        .unwrap();
    assert_eq!(b.rows.len(), 1);
    assert_eq!(b.rows[0][0], Value::from("SELECT 'x'"));
    assert_eq!(b.rows[0][1], Value::Int(1));
    assert_eq!(b.rows[0][2], Value::Int(1));
    assert_eq!(b.rows[0][3], Value::Int(15));
    assert_eq!(b.rows[0][4], Value::from("π"));
    assert_eq!(b.rows[0][5], Value::Null);
    let b = s
        .query("SELECT model, input_tokens, output_tokens, memo_hit FROM trace_calls")
        .await
        .unwrap();
    assert_eq!(
        b.rows,
        vec![vec![
            Value::from("m"),
            Value::Int(10),
            Value::Int(5),
            Value::Bool(false)
        ]]
    );
    for (table, n) in [
        ("trace_runs", 1),
        ("trace_sessions", 1),
        ("trace_tool_calls", 1),
        ("trace_rounds", 1),
        ("trace_final", 1),
    ] {
        let b = s
            .query(&format!("SELECT COUNT(*) FROM {table}"))
            .await
            .unwrap();
        assert_eq!(b.rows[0][0], Value::Int(n), "{table}");
    }
    let b = s
        .query("SELECT depth, role FROM trace_sessions")
        .await
        .unwrap();
    assert_eq!(b.rows, vec![vec![Value::Int(0), Value::from("self")]]);
    // record() is non-blocking and safe from a plain sync context.
    sink.record(callgebra_trace::Traced {
        at: std::time::SystemTime::now(),
        event: TraceEvent::Final { session, rows: 2 },
    });
    sink.flush().await;
    let b = s.query("SELECT COUNT(*) FROM trace_final").await.unwrap();
    assert_eq!(b.rows[0][0], Value::Int(2));
}

#[tokio::test]
async fn memo_put_and_get() {
    let s = DuckDbStore::in_memory().unwrap();
    assert!(s.memo_get("m", "fp").await.unwrap().is_none());
    let e = MemoEntry {
        response: serde_json::json!({"text": "hi", "n": 1}),
        input_tokens: 3,
        output_tokens: 4,
        cost_usd: Some(0.5),
    };
    s.memo_put("m", "fp", &e).await.unwrap();
    assert_eq!(s.memo_get("m", "fp").await.unwrap(), Some(e.clone()));
    let e2 = MemoEntry {
        cost_usd: None,
        ..e
    };
    s.memo_put("m", "fp", &e2).await.unwrap();
    assert_eq!(s.memo_get("m", "fp").await.unwrap(), Some(e2));
    assert!(s.tables().await.unwrap().is_empty(), "memo is internal");
}

#[tokio::test]
async fn persists_to_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.duckdb");
    {
        let s = DuckDbStore::open(&path).unwrap();
        s.create_table("t", schema()).await.unwrap();
        s.insert(
            "t",
            callgebra_core::Batch::try_new(schema(), rows()).unwrap(),
        )
        .await
        .unwrap();
    }
    let s = DuckDbStore::open(&path).unwrap();
    assert_eq!(s.scan("t").await.unwrap().rows, rows());
}
