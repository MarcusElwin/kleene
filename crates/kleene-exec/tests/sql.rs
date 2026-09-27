//! End-to-end tests: CallSQL text -> planner -> executor over an in-memory sink.

use kleene_core::{
    standard_catalog, Catalog, DataType, Field, Schema, TableDef, TableSource, Value, Volatility,
};
use kleene_exec::{execute_statement, ExecContext, ExecError, MemorySink, StatementResult};
use kleene_sql::plan_sql;
use std::sync::Arc;

fn v(x: i64) -> Value {
    Value::Int(x)
}
fn f(x: f64) -> Value {
    Value::Float(x)
}
fn t(x: &str) -> Value {
    Value::from(x)
}
const N: Value = Value::Null;

/// (table name, columns, rows)
type Fixture = (&'static str, Vec<(&'static str, DataType)>, Vec<Vec<Value>>);

struct World {
    catalog: Catalog,
    sink: Arc<MemorySink>,
}

impl World {
    async fn new() -> Self {
        let mut catalog = standard_catalog();
        let sink = Arc::new(MemorySink::new());
        let tables: Vec<Fixture> = vec![
            (
                "edges",
                vec![
                    ("src", DataType::Int),
                    ("dst", DataType::Int),
                    ("w", DataType::Float),
                ],
                vec![
                    vec![v(1), v(2), f(1.0)],
                    vec![v(2), v(3), f(2.5)],
                    vec![v(3), v(1), f(0.5)],
                    vec![v(3), v(4), f(4.0)],
                    vec![v(5), v(6), N],
                ],
            ),
            (
                "nodes",
                vec![("id", DataType::Int), ("name", DataType::Text)],
                vec![
                    vec![v(1), t("a")],
                    vec![v(2), t("b")],
                    vec![v(3), t("c")],
                    vec![v(4), N],
                    vec![v(7), t("g")],
                ],
            ),
        ];
        for (name, cols, rows) in tables {
            let schema = Arc::new(Schema::new(
                cols.iter().map(|(n, ty)| Field::new(*n, *ty)).collect(),
            ));
            catalog.add_table(TableDef {
                name: name.into(),
                schema: (*schema).clone(),
                source: TableSource::Stored,
                volatility: Volatility::Stable,
                description: String::new(),
            });
            sink.load(name, schema, rows).await;
        }
        Self { catalog, sink }
    }

    async fn run(&self, sql: &str) -> Result<StatementResult, ExecError> {
        let stmts = plan_sql(sql, &self.catalog).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let mut last = None;
        for s in &stmts {
            last = Some(execute_statement(s, ExecContext::new(self.sink.clone())).await?);
        }
        Ok(last.expect("at least one statement"))
    }

    async fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        match self.run(sql).await.unwrap_or_else(|e| panic!("{sql}: {e}")) {
            StatementResult::Rows(b) => b.rows,
            other => panic!("{sql}: expected rows, got {other:?}"),
        }
    }

    async fn err(&self, sql: &str) -> ExecError {
        self.run(sql).await.expect_err(sql)
    }
}

#[tokio::test]
async fn filter_project_sort_with_nulls() {
    let w = World::new().await;
    let r = w
        .rows("SELECT id, name FROM nodes WHERE id < 5 ORDER BY name")
        .await;
    assert_eq!(
        r,
        vec![
            vec![v(1), t("a")],
            vec![v(2), t("b")],
            vec![v(3), t("c")],
            vec![v(4), N]
        ]
    );
    let r = w.rows("SELECT name FROM nodes ORDER BY name DESC").await;
    assert_eq!(r[0], vec![N], "DESC puts NULLs first by default");
    let r = w
        .rows("SELECT name FROM nodes ORDER BY name ASC NULLS FIRST LIMIT 1")
        .await;
    assert_eq!(r, vec![vec![N]]);
    let r = w
        .rows("SELECT id FROM nodes ORDER BY id LIMIT 2 OFFSET 1")
        .await;
    assert_eq!(r, vec![vec![v(2)], vec![v(3)]]);
    let r = w
        .rows("SELECT id * 2 AS d, name || '!' FROM nodes WHERE name IS NOT NULL ORDER BY d")
        .await;
    assert_eq!(r[0], vec![v(2), t("a!")]);
}

#[tokio::test]
async fn joins() {
    let w = World::new().await;
    let r = w
        .rows(
            "SELECT e.src, n.name FROM edges e JOIN nodes n ON n.id = e.dst ORDER BY e.src, n.name",
        )
        .await;
    assert_eq!(
        r,
        vec![
            vec![v(1), t("b")],
            vec![v(2), t("c")],
            vec![v(3), t("a")],
            vec![v(3), N]
        ]
    );
    let r = w
        .rows("SELECT n.id, e.dst FROM nodes n LEFT JOIN edges e ON e.src = n.id ORDER BY n.id, e.dst")
        .await;
    assert_eq!(r.len(), 6);
    assert_eq!(r[4], vec![v(4), N], "unmatched left rows are NULL-padded");
    assert_eq!(r[5], vec![v(7), N]);
    let r = w.rows("SELECT COUNT(*) FROM nodes, edges").await;
    assert_eq!(r, vec![vec![v(25)]]);
    let r = w.rows("SELECT COUNT(*) FROM nodes CROSS JOIN edges").await;
    assert_eq!(r, vec![vec![v(25)]]);
}

#[tokio::test]
async fn aggregates() {
    let w = World::new().await;
    let r = w
        .rows("SELECT src, COUNT(*), SUM(w), MIN(dst), MAX(w), AVG(w) FROM edges GROUP BY src ORDER BY src")
        .await;
    assert_eq!(r[2], vec![v(3), v(2), f(4.5), v(1), f(4.0), f(2.25)]);
    assert_eq!(
        r[3],
        vec![v(5), v(1), N, v(6), N, N],
        "all-NULL group sums to NULL"
    );
    let r = w
        .rows("SELECT COUNT(*), COUNT(name), COUNT(DISTINCT src) FROM nodes, edges")
        .await;
    assert_eq!(r, vec![vec![v(25), v(20), v(4)]]);
    let r = w
        .rows("SELECT COUNT(*), SUM(id), MAX(name) FROM nodes WHERE id > 100")
        .await;
    assert_eq!(
        r,
        vec![vec![v(0), N, N]],
        "empty input without GROUP BY gives one row"
    );
    let r = w
        .rows("SELECT id, COUNT(*) FROM nodes WHERE id > 100 GROUP BY id")
        .await;
    assert!(r.is_empty(), "empty input with GROUP BY gives no rows");
    let r = w
        .rows("SELECT src FROM edges GROUP BY src HAVING SUM(w) > 2 ORDER BY src")
        .await;
    assert_eq!(r, vec![vec![v(2)], vec![v(3)]]);
    let r = w.rows("SELECT STRING_AGG(name, ',') FROM (SELECT name FROM nodes WHERE name IS NOT NULL ORDER BY name) s").await;
    assert_eq!(r, vec![vec![t("a,b,c,g")]]);
    // ORDER BY inside the call, the way a model writes it.
    let r = w
        .rows("SELECT STRING_AGG(name, ',' ORDER BY id DESC) FROM nodes")
        .await;
    assert_eq!(r, vec![vec![t("g,c,b,a")]]);
    let r = w
        .rows("SELECT src, STRING_AGG(CAST(dst AS TEXT), '>' ORDER BY w DESC NULLS LAST) FROM edges GROUP BY src ORDER BY src")
        .await;
    assert_eq!(
        r,
        vec![
            vec![v(1), t("2")],
            vec![v(2), t("3")],
            vec![v(3), t("4>1")],
            vec![v(5), t("6")],
        ]
    );
    let r = w
        .rows("SELECT STRING_AGG(DISTINCT name, ',' ORDER BY name) FROM nodes")
        .await;
    assert_eq!(r, vec![vec![t("a,b,c,g")]]);
    let r = w
        .rows("SELECT BOOL_AND(id > 0), BOOL_OR(id > 6) FROM nodes")
        .await;
    assert_eq!(r, vec![vec![Value::Bool(true), Value::Bool(true)]]);
    let r = w.rows("SELECT SUM(w) FROM edges").await;
    assert_eq!(r, vec![vec![f(8.0)]]);
    let r = w.rows("SELECT SUM(id) FROM nodes").await;
    assert_eq!(r, vec![vec![v(17)]], "integer sums stay integers");
}

#[tokio::test]
async fn distinct_union_limit() {
    let w = World::new().await;
    let r = w.rows("SELECT DISTINCT src FROM edges ORDER BY src").await;
    assert_eq!(r, vec![vec![v(1)], vec![v(2)], vec![v(3)], vec![v(5)]]);
    let r = w
        .rows("SELECT src FROM edges UNION SELECT dst FROM edges ORDER BY 1")
        .await;
    assert_eq!(r.len(), 6);
    let r = w
        .rows("SELECT src FROM edges UNION ALL SELECT dst FROM edges")
        .await;
    assert_eq!(r.len(), 10);
    let r = w
        .rows("SELECT DISTINCT name FROM nodes WHERE name IS NULL OR id = 4")
        .await;
    assert_eq!(r, vec![vec![N]], "DISTINCT treats NULLs as equal");
}

#[tokio::test]
async fn three_valued_logic_and_arithmetic() {
    let w = World::new().await;
    let r = w
        .rows("SELECT TRUE AND NULL, FALSE AND NULL, TRUE OR NULL, FALSE OR NULL, NOT NULL, NULL = NULL, 1 = NULL")
        .await;
    assert_eq!(
        r,
        vec![vec![N, Value::Bool(false), Value::Bool(true), N, N, N, N]]
    );
    let r = w
        .rows("SELECT 7 / 2, 7 % 3, 1 / 0, 2 * 2.5, -3, 1 + NULL")
        .await;
    assert_eq!(
        r,
        vec![vec![f(3.5), v(1), f(f64::INFINITY), f(5.0), v(-3), N]]
    );
    let e = w.err("SELECT 9223372036854775807 + 1").await;
    assert!(matches!(e, ExecError::Eval(_)), "{e}");
    // Division and modulo by zero follow DuckDB: IEEE floats, NULL modulo.
    let r = w.rows("SELECT 1 % 0, 0 / 0 IS NULL, -1 / 0").await;
    assert_eq!(r, vec![vec![N, Value::Bool(false), f(f64::NEG_INFINITY)]]);
    let r = w
        .rows("SELECT name FROM nodes WHERE name LIKE '_' AND name NOT LIKE 'g' ORDER BY name")
        .await;
    assert_eq!(r, vec![vec![t("a")], vec![t("b")], vec![t("c")]]);
    let r = w.rows("SELECT 'AbC' ILIKE 'a%', 'x' LIKE NULL").await;
    assert_eq!(r, vec![vec![Value::Bool(true), N]]);
    let r = w
        .rows("SELECT CASE WHEN id > 3 THEN 'big' WHEN id > 1 THEN 'mid' ELSE 'small' END FROM nodes ORDER BY id")
        .await;
    assert_eq!(r[0], vec![t("small")]);
    assert_eq!(r[1], vec![t("mid")]);
    assert_eq!(r[3], vec![t("big")]);
    let r = w
        .rows("SELECT CASE id WHEN 1 THEN 'one' END FROM nodes ORDER BY id LIMIT 2")
        .await;
    assert_eq!(r, vec![vec![t("one")], vec![N]]);
    let r = w
        .rows("SELECT 1 IN (1, NULL), 2 IN (1, NULL), 2 IN (1, 3), 2 NOT IN (1, 3), NULL IN (1)")
        .await;
    assert_eq!(
        r,
        vec![vec![
            Value::Bool(true),
            N,
            Value::Bool(false),
            Value::Bool(true),
            N
        ]]
    );
    let r = w
        .rows("SELECT CAST('42' AS BIGINT), CAST(1 AS TEXT), CAST(3.0 AS BIGINT), 1::DOUBLE")
        .await;
    assert_eq!(r, vec![vec![v(42), t("1"), v(3), f(1.0)]]);
    let e = w.err("SELECT CAST('x' AS BIGINT)").await;
    assert!(matches!(e, ExecError::Eval(_)));
    let r = w
        .rows("SELECT id BETWEEN 2 AND 3 FROM nodes ORDER BY id LIMIT 3")
        .await;
    assert_eq!(
        r,
        vec![
            vec![Value::Bool(false)],
            vec![Value::Bool(true)],
            vec![Value::Bool(true)]
        ]
    );
}

#[tokio::test]
async fn builtins_through_sql() {
    let w = World::new().await;
    let r = w
        .rows("SELECT upper(name), length(name), coalesce(name, 'none'), nullif(id, 4), greatest(id, 3) FROM nodes WHERE id IN (1, 4) ORDER BY id")
        .await;
    assert_eq!(r[0], vec![t("A"), v(1), t("a"), v(1), v(3)]);
    assert_eq!(r[1], vec![N, N, t("none"), N, v(4)]);
    let r = w.rows("SELECT substr('hello', 2, 3), concat('a', NULL, 1), replace('aXa', 'X', 'b'), abs(-2), round(2.567, 2), floor(1.5), ceil(1.5), mod(7, 3)").await;
    assert_eq!(
        r,
        vec![vec![
            t("ell"),
            t("a1"),
            t("aba"),
            v(2),
            f(2.57),
            f(1.0),
            f(2.0),
            v(1)
        ]]
    );
    let r = w.rows("SELECT trim('  x '), ltrim(' y'), rtrim('z '), starts_with('abc', 'ab'), contains('abc', 'z'), split_part('a-b', '-', 2)").await;
    assert_eq!(
        r,
        vec![vec![
            t("x"),
            t("y"),
            t("z"),
            Value::Bool(true),
            Value::Bool(false),
            t("b")
        ]]
    );
    let r = w
        .rows("SELECT json_extract_string(CAST('{\"a\":{\"b\":\"c\"}}' AS JSON), '$.a.b')")
        .await;
    assert_eq!(r, vec![vec![t("c")]]);
}

#[tokio::test]
async fn generate_series_plain_and_lateral() {
    let w = World::new().await;
    let r = w
        .rows("SELECT generate_series FROM generate_series(3, 1, -1)")
        .await;
    assert_eq!(r, vec![vec![v(3)], vec![v(2)], vec![v(1)]]);
    let r = w
        .rows("SELECT n.id, g.generate_series FROM nodes n CROSS JOIN LATERAL generate_series(1, n.id) g WHERE n.id <= 2 ORDER BY 1, 2")
        .await;
    assert_eq!(
        r,
        vec![vec![v(1), v(1)], vec![v(2), v(1)], vec![v(2), v(2)]]
    );
    let e = w.err("SELECT * FROM generate_series(1, 5, 0)").await;
    assert!(matches!(e, ExecError::Eval(_)));
}

#[tokio::test]
async fn correlated_subqueries() {
    let w = World::new().await;
    let r = w
        .rows("SELECT n.id FROM nodes n WHERE NOT EXISTS (SELECT 1 FROM edges e WHERE e.src = n.id) ORDER BY n.id")
        .await;
    assert_eq!(r, vec![vec![v(4)], vec![v(7)]]);
    let r = w
        .rows("SELECT n.id FROM nodes n WHERE EXISTS (SELECT 1 FROM edges e WHERE e.dst = n.id AND e.w > 2) ORDER BY n.id")
        .await;
    assert_eq!(r, vec![vec![v(3)], vec![v(4)]]);
    let r = w
        .rows("SELECT id, (SELECT MAX(w) FROM edges WHERE src = nodes.id) FROM nodes ORDER BY id")
        .await;
    assert_eq!(r[2], vec![v(3), f(4.0)]);
    assert_eq!(r[3], vec![v(4), N]);
    let r = w
        .rows("SELECT id FROM nodes WHERE id IN (SELECT dst FROM edges WHERE w IS NULL)")
        .await;
    assert!(r.is_empty(), "6 is not a node");
    let r = w
        .rows("SELECT id FROM nodes WHERE id NOT IN (SELECT src FROM edges) ORDER BY id")
        .await;
    assert_eq!(r, vec![vec![v(4)], vec![v(7)]]);
    let e = w.err("SELECT (SELECT src FROM edges) FROM nodes").await;
    assert!(
        matches!(e, ExecError::Eval(_)),
        "multi-row scalar subquery errors"
    );
    // Two levels of correlation.
    let r = w
        .rows("SELECT n.id FROM nodes n WHERE EXISTS (SELECT 1 FROM edges e WHERE e.src = n.id AND EXISTS (SELECT 1 FROM nodes m WHERE m.id = e.dst AND m.name IS NULL)) ORDER BY 1")
        .await;
    assert_eq!(r, vec![vec![v(3)]]);
}

#[tokio::test]
async fn ctes_and_recursion() {
    let w = World::new().await;
    let r = w.rows("WITH a AS (SELECT src FROM edges), b AS (SELECT src + 1 AS s FROM a) SELECT COUNT(*) FROM b").await;
    assert_eq!(r, vec![vec![v(5)]]);
    // Transitive closure from node 1 over a cycle 1->2->3->1 plus 3->4: set semantics terminate.
    let r = w
        .rows("WITH RECURSIVE reach(n) AS (SELECT dst FROM edges WHERE src = 1 UNION SELECT e.dst FROM reach r JOIN edges e ON e.src = r.n) SELECT n FROM reach ORDER BY n")
        .await;
    assert_eq!(r, vec![vec![v(1)], vec![v(2)], vec![v(3)], vec![v(4)]]);
    // Shortest hop count with a depth column and a bound.
    let r = w
        .rows("WITH RECURSIVE p(n, d) AS (SELECT 1, 0 UNION ALL SELECT e.dst, p.d + 1 FROM p JOIN edges e ON e.src = p.n WHERE p.d < 3) SELECT n, MIN(d) FROM p GROUP BY n ORDER BY n")
        .await;
    assert_eq!(
        r,
        vec![
            vec![v(1), v(0)],
            vec![v(2), v(1)],
            vec![v(3), v(2)],
            vec![v(4), v(3)]
        ]
    );
    // UNION ALL without a bound would loop forever on the cycle; the context cap stops it.
    let stmts = plan_sql(
        "WITH RECURSIVE p(n) AS (SELECT 1 UNION ALL SELECT e.dst FROM p JOIN edges e ON e.src = p.n) SELECT COUNT(*) FROM p",
        &w.catalog,
    )
    .unwrap();
    let mut ctx = ExecContext::new(w.sink.clone());
    ctx.max_recursion_rounds = Some(5);
    let res = execute_statement(&stmts[0], ctx).await.unwrap();
    let StatementResult::Rows(b) = res else {
        panic!()
    };
    // rounds: {1}, {2}, {3}, {1,4}, {2}, {3} -> 1+1+1+2+1+1 = 7 rows
    assert_eq!(b.rows, vec![vec![v(7)]]);
}

#[tokio::test]
async fn statements_create_insert_drop_final() {
    let w = World::new().await;
    let r = w
        .run("CREATE TABLE big AS SELECT src, dst FROM edges WHERE w > 1")
        .await
        .unwrap();
    assert!(matches!(r, StatementResult::Affected { rows: 2, .. }));
    assert_eq!(w.sink.rows("big").await.unwrap().len(), 2);
    // The catalog does not know `big` (the harness registers new tables in M3), so insert via the sink directly.
    let e = w.run("CREATE TABLE big AS SELECT 1").await.unwrap_err();
    assert!(matches!(e, ExecError::TableExists(_)));
    let r = w
        .run("CREATE TABLE IF NOT EXISTS big AS SELECT 1")
        .await
        .unwrap();
    assert!(matches!(r, StatementResult::Affected { rows: 0, .. }));
    assert_eq!(
        w.sink.rows("big").await.unwrap().len(),
        2,
        "existing table untouched"
    );
    let r = w
        .run("INSERT INTO nodes (name, id) VALUES ('h', 8), ('i', 9)")
        .await
        .unwrap();
    assert!(matches!(r, StatementResult::Affected { rows: 2, .. }));
    let r = w
        .rows("SELECT id, name FROM nodes WHERE id > 7 ORDER BY id")
        .await;
    assert_eq!(r, vec![vec![v(8), t("h")], vec![v(9), t("i")]]);
    let r = w
        .run("INSERT INTO nodes SELECT id + 100, name FROM nodes WHERE id = 1")
        .await
        .unwrap();
    assert!(matches!(r, StatementResult::Affected { rows: 1, .. }));
    let r = w.run("DROP TABLE nodes").await.unwrap();
    assert!(matches!(r, StatementResult::Affected { .. }));
    assert!(w.sink.rows("nodes").await.is_none());
    let e = w.run("DROP TABLE nodes").await.unwrap_err();
    assert!(matches!(e, ExecError::NoSuchTable(_)));
    w.run("DROP TABLE IF EXISTS nodes").await.unwrap();
    let r = w.run("FINAL('done')").await.unwrap();
    let StatementResult::Final(b) = r else {
        panic!("{r:?}")
    };
    assert_eq!(b.rows, vec![vec![t("done")]]);
    assert_eq!(b.schema.names(), ["answer"]);
    let r = w.run("SET budget.calls = 10").await.unwrap();
    assert!(matches!(r, StatementResult::Set { .. }));
}

#[tokio::test]
async fn values_and_empty_from() {
    let w = World::new().await;
    let r = w.rows("SELECT 1 AS x, 'a' AS y").await;
    assert_eq!(r, vec![vec![v(1), t("a")]]);
    let r = w
        .rows("SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t(n, s) ORDER BY n DESC")
        .await;
    assert_eq!(r, vec![vec![v(2), t("b")], vec![v(1), t("a")]]);
}

/// A sink that answers `tag(x)` in batches and counts how it was asked.
struct BatchSink {
    inner: MemorySink,
    batch: Option<usize>,
    single_calls: std::sync::atomic::AtomicUsize,
    batch_calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl kleene_exec::CallSink for BatchSink {
    async fn scalar_call(&self, name: &str, args: &[Value]) -> Result<Value, ExecError> {
        self.single_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Value::Text(format!("{name}:{}", args[0].render())))
    }
    async fn batch_size(&self, name: &str) -> Option<usize> {
        (name == "tag").then_some(self.batch?)
    }
    async fn scalar_call_batch(
        &self,
        name: &str,
        tuples: &[Vec<Value>],
    ) -> Result<Vec<Value>, ExecError> {
        self.batch_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(tuples
            .iter()
            .map(|t| Value::Text(format!("{name}:{}", t[0].render())))
            .collect())
    }
    async fn table_call(
        &self,
        name: &str,
        args: &[Value],
    ) -> Result<kleene_core::Batch, ExecError> {
        self.inner.table_call(name, args).await
    }
    async fn scan(&self, table: &str) -> Result<kleene_exec::BatchStream, ExecError> {
        self.inner.scan(table).await
    }
    async fn create_table(
        &self,
        name: &str,
        schema: Arc<Schema>,
        if_not_exists: bool,
    ) -> Result<bool, ExecError> {
        self.inner.create_table(name, schema, if_not_exists).await
    }
    async fn insert(&self, name: &str, batch: kleene_core::Batch) -> Result<(), ExecError> {
        self.inner.insert(name, batch).await
    }
    async fn drop_table(&self, name: &str, if_exists: bool) -> Result<(), ExecError> {
        self.inner.drop_table(name, if_exists).await
    }
}

async fn batch_world(batch: Option<usize>) -> (Catalog, Arc<BatchSink>) {
    let mut catalog = standard_catalog();
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int),
        Field::new("name", DataType::Text),
    ]));
    catalog.add_table(TableDef {
        name: "items".into(),
        schema: (*schema).clone(),
        source: TableSource::Stored,
        volatility: Volatility::Stable,
        description: String::new(),
    });
    catalog.add_function(kleene_core::FunctionDef {
        name: "tag".into(),
        args: vec![DataType::Text],
        variadic: false,
        returns: kleene_core::FunctionReturn::Scalar {
            data_type: DataType::Text,
        },
        call_kind: kleene_core::CallKind::LlmScalar {
            alias: kleene_core::ModelAlias::worker(),
            batch,
        },
        volatility: Volatility::Immutable,
        description: String::new(),
    });
    let inner = MemorySink::new();
    inner
        .load(
            "items",
            schema,
            vec![
                vec![v(1), t("a")],
                vec![v(2), t("b")],
                vec![v(3), t("a")],
                vec![v(4), N],
                vec![v(5), t("c")],
            ],
        )
        .await;
    (
        catalog,
        Arc::new(BatchSink {
            inner,
            batch,
            single_calls: Default::default(),
            batch_calls: Default::default(),
        }),
    )
}

async fn batch_rows(catalog: &Catalog, sink: Arc<BatchSink>, sql: &str) -> Vec<Vec<Value>> {
    let stmts = plan_sql(sql, catalog).unwrap();
    match execute_statement(&stmts[0], ExecContext::new(sink))
        .await
        .unwrap()
    {
        StatementResult::Rows(b) => b.rows,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn batchable_calls_are_folded_per_distinct_tuple() {
    use std::sync::atomic::Ordering::SeqCst;
    let sql = "SELECT id, tag(name) FROM items ORDER BY id";
    let expect = vec![
        vec![v(1), t("tag:a")],
        vec![v(2), t("tag:b")],
        vec![v(3), t("tag:a")],
        vec![v(4), t("tag:NULL")],
        vec![v(5), t("tag:c")],
    ];
    // Batch of 2 over three distinct non-null names: two batched calls, and
    // the null row still goes through the ordinary path.
    let (catalog, sink) = batch_world(Some(2)).await;
    assert_eq!(batch_rows(&catalog, sink.clone(), sql).await, expect);
    assert_eq!(sink.batch_calls.load(SeqCst), 2);
    assert_eq!(sink.single_calls.load(SeqCst), 1);
    // Without a batch size nothing changes: one call per row.
    let (catalog, sink) = batch_world(None).await;
    assert_eq!(batch_rows(&catalog, sink.clone(), sql).await, expect);
    assert_eq!(sink.batch_calls.load(SeqCst), 0);
    assert_eq!(sink.single_calls.load(SeqCst), 5);
    // A predicate is batched the same way, and a call nested inside a call
    // argument is left alone.
    let (catalog, sink) = batch_world(Some(10)).await;
    let r = batch_rows(
        &catalog,
        sink.clone(),
        "SELECT id FROM items WHERE tag(name) = 'tag:a' ORDER BY id",
    )
    .await;
    assert_eq!(r, vec![vec![v(1)], vec![v(3)]]);
    assert_eq!(sink.batch_calls.load(SeqCst), 1);
    let (catalog, sink) = batch_world(Some(10)).await;
    batch_rows(&catalog, sink.clone(), "SELECT tag(tag(name)) FROM items").await;
    assert_eq!(sink.batch_calls.load(SeqCst), 1, "inner call batched");
    assert_eq!(
        sink.single_calls.load(SeqCst),
        6,
        "outer call per row, plus the inner call on the null name"
    );
}
