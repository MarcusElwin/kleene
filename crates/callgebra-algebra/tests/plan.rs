//! Call-plan annotation, rules and EXPLAIN over a fixed catalog.

use callgebra_algebra::{explain, plan, plan_with, CostModel, Fragment, Rule};
use callgebra_core::{
    standard_catalog, CallKind, Catalog, DataType, Field, Schema, TableDef, TableSource, Value,
    Volatility,
};
use callgebra_exec::{execute_statement, ExecContext, MemorySink, StatementResult};
use callgebra_sql::{plan_sql, JoinKind, LogicalPlan, StatementKind};
use std::sync::Arc;

fn catalog() -> Catalog {
    let mut c = standard_catalog();
    let table = |name: &str, cols: &[(&str, DataType)]| TableDef {
        name: name.into(),
        schema: Schema::new(cols.iter().map(|(n, t)| Field::new(*n, *t)).collect()),
        source: TableSource::Stored,
        volatility: Volatility::Stable,
        description: String::new(),
    };
    c.add_table(table(
        "possibilities",
        &[("candidate", DataType::Text), ("score", DataType::Float)],
    ));
    c.add_table(table(
        "counterexamples",
        &[("text", DataType::Text), ("kind", DataType::Text)],
    ));
    let prompt_fn = |name: &str, args: Vec<DataType>| callgebra_core::FunctionDef {
        name: name.into(),
        args,
        variadic: false,
        returns: callgebra_core::FunctionReturn::Scalar {
            data_type: DataType::Bool,
        },
        call_kind: CallKind::LlmScalar {
            alias: callgebra_core::ModelAlias::worker(),
        },
        volatility: Volatility::Immutable,
        description: String::new(),
    };
    c.add_function(prompt_fn("verify", vec![DataType::Text]));
    c.add_function(prompt_fn("refute", vec![DataType::Text, DataType::Text]));
    c.add_function(callgebra_core::FunctionDef {
        name: "shell_flag".into(),
        args: vec![DataType::Text],
        variadic: false,
        returns: callgebra_core::FunctionReturn::Scalar {
            data_type: DataType::Bool,
        },
        call_kind: CallKind::Tool {
            tool: "shell".into(),
        },
        volatility: Volatility::Volatile,
        description: String::new(),
    });
    c
}

fn logical(sql: &str, c: &Catalog) -> LogicalPlan {
    match plan_sql(sql, c).unwrap().remove(0).kind {
        StatementKind::Query { plan } => plan,
        other => panic!("{other:?}"),
    }
}

const PITCH: &str = "SELECT candidate FROM possibilities WHERE verify(candidate) AND NOT EXISTS (SELECT 1 FROM counterexamples ce WHERE refute(candidate, ce.text))";

#[test]
fn annotation_counts_calls_and_estimates_cost() {
    let c = catalog();
    let mut cost = CostModel::default();
    cost.table_rows.insert("possibilities".into(), 40.0);
    cost.table_rows.insert("counterexamples".into(), 5.0);
    let cp = plan_with(&logical(PITCH, &c), &c, &cost, &[]);
    let text = explain(&cp);
    assert!(text.contains("λ worker"), "{text}");
    // Conjuncts are priced left to right the way the executor evaluates
    // them: verify on 40 rows, then NOT EXISTS on the 20 survivors (default
    // selectivity 0.5) × 5 counterexamples = 100 calls.
    assert!(
        (cp.total.calls - 140.0).abs() < 1e-9,
        "{}: {text}",
        cp.total.calls
    );
    assert!(cp.total.dollars > 0.0);
    assert_eq!(cp.fragment, Fragment::FirstOrder);
    assert!(text.contains("fragment: FO"));
}

#[test]
fn semi_join_rule_rewrites_not_exists_and_keeps_semantics() {
    let c = catalog();
    let cost = CostModel::default();
    let cp = plan(&logical(PITCH, &c), &c, &cost);
    assert!(
        cp.rules_applied.iter().any(|r| r == "semi-join"),
        "{:?}",
        cp.rules_applied
    );
    let rewritten = cp.logical();
    let mut found = false;
    fn walk(p: &LogicalPlan, found: &mut bool) {
        if let LogicalPlan::Join {
            kind: JoinKind::Anti,
            ..
        } = p
        {
            *found = true;
        }
        for ch in p.children() {
            walk(ch, found);
        }
    }
    walk(&rewritten, &mut found);
    assert!(found, "{}", explain(&cp));

    // Execute original and rewritten plans over the same data with a sink
    // that answers verify/refute deterministically.
    struct Oracle(MemorySink);
    #[async_trait::async_trait]
    impl callgebra_exec::CallSink for Oracle {
        async fn scalar_call(
            &self,
            name: &str,
            args: &[Value],
        ) -> Result<Value, callgebra_exec::ExecError> {
            let a = args[0].render();
            Ok(match name {
                "verify" => Value::Bool(!a.contains("bad")),
                "refute" => Value::Bool(args[1].render().contains(&a)),
                _ => Value::Null,
            })
        }
        async fn table_call(
            &self,
            n: &str,
            a: &[Value],
        ) -> Result<callgebra_core::Batch, callgebra_exec::ExecError> {
            self.0.table_call(n, a).await
        }
        async fn scan(
            &self,
            t: &str,
        ) -> Result<callgebra_exec::BatchStream, callgebra_exec::ExecError> {
            self.0.scan(t).await
        }
        async fn create_table(
            &self,
            n: &str,
            s: Arc<Schema>,
            i: bool,
        ) -> Result<bool, callgebra_exec::ExecError> {
            self.0.create_table(n, s, i).await
        }
        async fn insert(
            &self,
            n: &str,
            b: callgebra_core::Batch,
        ) -> Result<(), callgebra_exec::ExecError> {
            self.0.insert(n, b).await
        }
        async fn drop_table(&self, n: &str, i: bool) -> Result<(), callgebra_exec::ExecError> {
            self.0.drop_table(n, i).await
        }
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let sink = MemorySink::new();
        let t = |name: &str| Arc::new(c.table(name).unwrap().schema.clone());
        sink.load(
            "possibilities",
            t("possibilities"),
            vec![
                vec![Value::from("alpha"), Value::Float(1.0)],
                vec![Value::from("bad idea"), Value::Float(0.5)],
                vec![Value::from("gamma"), Value::Float(0.9)],
            ],
        )
        .await;
        sink.load(
            "counterexamples",
            t("counterexamples"),
            vec![vec![Value::from("this refutes gamma"), Value::from("x")]],
        )
        .await;
        let sink = Arc::new(Oracle(sink));
        let run = |p: LogicalPlan| {
            let sink = sink.clone();
            async move {
                let stmt = callgebra_sql::Statement {
                    sql: String::new(),
                    kind: StatementKind::Query { plan: p },
                };
                match execute_statement(&stmt, ExecContext::new(sink))
                    .await
                    .unwrap()
                {
                    StatementResult::Rows(b) => b.rows,
                    other => panic!("{other:?}"),
                }
            }
        };
        let a = run(logical(PITCH, &c)).await;
        let b = run(rewritten).await;
        assert_eq!(a, vec![vec![Value::from("alpha")]]);
        assert_eq!(a, b);
    });
}

#[test]
fn cheap_first_orders_conjuncts() {
    let c = catalog();
    let cost = CostModel::default();
    let sql = "SELECT candidate FROM possibilities WHERE verify(candidate) AND score > 0.5";
    let cp = plan_with(
        &logical(sql, &c),
        &c,
        &cost,
        &[Box::new(callgebra_algebra::CheapFirst) as Box<dyn Rule>],
    );
    assert_eq!(cp.rules_applied, vec!["cheap-first"]);
    let text = explain(&cp);
    let pos_score = text.find("score > 0.5").unwrap();
    let pos_verify = text.find("verify(").unwrap();
    assert!(pos_score < pos_verify, "{text}");
}

#[test]
fn volatile_scalar_functions_are_refused_by_the_frontend() {
    let c = catalog();
    let e = plan_sql("SELECT shell_flag(candidate) FROM possibilities", &c).unwrap_err();
    assert!(e.to_string().contains("shell_flag"), "{e}");
}

#[test]
fn fragments() {
    let c = catalog();
    let cost = CostModel::default();
    let cq = plan(&logical("SELECT p.candidate FROM possibilities p JOIN counterexamples x ON x.text = p.candidate WHERE p.score > 1", &c), &c, &cost);
    assert_eq!(cq.fragment, Fragment::Conjunctive);
    let fo = plan(
        &logical("SELECT COUNT(*) FROM possibilities", &c),
        &c,
        &cost,
    );
    assert_eq!(fo.fragment, Fragment::FirstOrder);
    let rec = plan(&logical("WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 3) SELECT n FROM r", &c), &c, &cost);
    assert_eq!(rec.fragment, Fragment::Recursive);
    assert!(explain(&rec).contains("μ r"));
}

#[test]
fn expand_and_recursion_are_estimated_per_round() {
    let c = catalog();
    let mut cost = CostModel::default();
    cost.table_rows.insert("possibilities".into(), 4.0);
    cost.branching = 3.0;
    cost.recursion_rounds = 3.0;
    let sql = "WITH RECURSIVE f(h, d) AS (SELECT candidate, 0 FROM possibilities UNION ALL SELECT e.item, f.d + 1 FROM f CROSS JOIN LATERAL expand(f.h, 3) AS e WHERE f.d < 2) SELECT h FROM f";
    let cp = plan(&logical(sql, &c), &c, &cost);
    assert!(cp.total.calls > 0.0);
    let text = explain(&cp);
    assert!(text.contains("κ lateral expand"), "{text}");
    assert!(text.contains("κ worker"), "{text}");
}

/// Catalog for the join-order demo: three tables and two LLM predicates.
fn join_catalog() -> Catalog {
    let mut c = standard_catalog();
    let table = |name: &str, cols: &[(&str, DataType)]| TableDef {
        name: name.into(),
        schema: Schema::new(cols.iter().map(|(n, t)| Field::new(*n, *t)).collect()),
        source: TableSource::Stored,
        volatility: Volatility::Stable,
        description: String::new(),
    };
    c.add_table(table(
        "papers",
        &[("id", DataType::Int), ("title", DataType::Text)],
    ));
    c.add_table(table(
        "claims",
        &[
            ("id", DataType::Int),
            ("paper_id", DataType::Int),
            ("text", DataType::Text),
        ],
    ));
    c.add_table(table(
        "evidence",
        &[("claim_id", DataType::Int), ("snippet", DataType::Text)],
    ));
    let llm_bool = |name: &str, args: Vec<DataType>, alias: &str| callgebra_core::FunctionDef {
        name: name.into(),
        args,
        variadic: false,
        returns: callgebra_core::FunctionReturn::Scalar {
            data_type: DataType::Bool,
        },
        call_kind: CallKind::LlmScalar {
            alias: callgebra_core::ModelAlias(alias.into()),
        },
        volatility: Volatility::Immutable,
        description: String::new(),
    };
    c.add_function(llm_bool("relevant", vec![DataType::Text], "worker"));
    c.add_function(llm_bool(
        "supports",
        vec![DataType::Text, DataType::Text],
        "worker",
    ));
    c.add_function(callgebra_core::FunctionDef {
        name: "relevance_score".into(),
        args: vec![DataType::Text],
        variadic: false,
        returns: callgebra_core::FunctionReturn::Scalar {
            data_type: DataType::Float,
        },
        call_kind: CallKind::LlmScalar {
            alias: callgebra_core::ModelAlias::proxy(),
        },
        volatility: Volatility::Immutable,
        description: String::new(),
    });
    c
}

/// A sink that answers the demo predicates deterministically and counts
/// every call, so plans can be compared by what they actually spend.
struct Counting {
    inner: MemorySink,
    calls: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl callgebra_exec::CallSink for Counting {
    async fn scalar_call(
        &self,
        name: &str,
        args: &[Value],
    ) -> Result<Value, callgebra_exec::ExecError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let a = args[0].render();
        Ok(match name {
            "relevant" => Value::Bool(a.contains("graph")),
            "supports" => Value::Bool(args[1].render().contains(&a[..a.len().min(4)])),
            "relevance_score" => Value::Float(if a.contains("graph") {
                0.9
            } else if a.contains("tree") {
                0.5
            } else {
                0.1
            }),
            _ => Value::Null,
        })
    }
    async fn table_call(
        &self,
        n: &str,
        a: &[Value],
    ) -> Result<callgebra_core::Batch, callgebra_exec::ExecError> {
        self.inner.table_call(n, a).await
    }
    async fn scan(
        &self,
        t: &str,
    ) -> Result<callgebra_exec::BatchStream, callgebra_exec::ExecError> {
        self.inner.scan(t).await
    }
    async fn create_table(
        &self,
        n: &str,
        s: Arc<Schema>,
        i: bool,
    ) -> Result<bool, callgebra_exec::ExecError> {
        self.inner.create_table(n, s, i).await
    }
    async fn insert(
        &self,
        n: &str,
        b: callgebra_core::Batch,
    ) -> Result<(), callgebra_exec::ExecError> {
        self.inner.insert(n, b).await
    }
    async fn drop_table(&self, n: &str, i: bool) -> Result<(), callgebra_exec::ExecError> {
        self.inner.drop_table(n, i).await
    }
}

async fn demo_sink(c: &Catalog, n: usize) -> Arc<Counting> {
    let sink = MemorySink::new();
    let t = |name: &str| Arc::new(c.table(name).unwrap().schema.clone());
    let topics = ["graph", "tree", "sort", "hash"];
    sink.load(
        "papers",
        t("papers"),
        (0..n)
            .map(|i| {
                vec![
                    Value::Int(i as i64),
                    Value::from(format!("{} paper {i}", topics[i % 4])),
                ]
            })
            .collect(),
    )
    .await;
    sink.load(
        "claims",
        t("claims"),
        (0..n)
            .map(|i| {
                vec![
                    Value::Int(i as i64),
                    Value::Int(((i * 7) % n) as i64),
                    Value::from(format!("{} claim {i}", topics[(i / 2) % 4])),
                ]
            })
            .collect(),
    )
    .await;
    sink.load(
        "evidence",
        t("evidence"),
        (0..n)
            .map(|i| {
                vec![
                    Value::Int(((i * 3) % n) as i64),
                    Value::from(format!("{} snippet {i}", topics[(i / 3) % 4])),
                ]
            })
            .collect(),
    )
    .await;
    Arc::new(Counting {
        inner: sink,
        calls: std::sync::atomic::AtomicU64::new(0),
    })
}

async fn run_plan(sink: &Arc<Counting>, p: LogicalPlan) -> (Vec<Vec<Value>>, u64) {
    sink.calls.store(0, std::sync::atomic::Ordering::SeqCst);
    let stmt = callgebra_sql::Statement {
        sql: String::new(),
        kind: StatementKind::Query { plan: p },
    };
    let rows = match execute_statement(&stmt, ExecContext::new(sink.clone()))
        .await
        .unwrap()
    {
        StatementResult::Rows(b) => b.rows,
        other => panic!("{other:?}"),
    };
    let mut rows = rows;
    rows.sort_by_key(|r| r.iter().map(Value::render).collect::<Vec<_>>());
    (rows, sink.calls.load(std::sync::atomic::Ordering::SeqCst))
}

const THREE_WAY: &str = "SELECT p.title, c.text, e.snippet FROM papers p, claims c, evidence e \
WHERE supports(c.text, e.snippet) AND relevant(p.title) AND c.paper_id = p.id AND e.claim_id = c.id";

#[test]
fn join_order_picks_a_plan_an_order_of_magnitude_cheaper_and_keeps_semantics() {
    let c = join_catalog();
    let mut cost = CostModel::default();
    for t in ["papers", "claims", "evidence"] {
        cost.table_rows.insert(t.into(), 20.0);
    }
    let written = plan_with(&logical(THREE_WAY, &c), &c, &cost, &[]);
    let chosen = plan(&logical(THREE_WAY, &c), &c, &cost);
    let text = explain(&chosen);
    assert!(
        chosen.rules_applied.iter().any(|r| r == "join-order"),
        "{text}"
    );
    assert!(
        written.total.calls >= 10.0 * chosen.total.calls,
        "written {} vs chosen {}\n{text}",
        written.total.calls,
        chosen.total.calls
    );
    let ps = chosen.plan_space.expect("plan space reported");
    assert_eq!(ps.relations, 3);
    assert_eq!(ps.orders, 12, "3! × C(2) bushy trees");
    assert!(ps.evaluated > 0);
    assert!(text.contains("plan space: 3 relations"), "{text}");
    assert!(text.contains("alternatives:"), "{text}");
    assert!(text.contains("as written"), "{text}");
    assert_eq!(chosen.alternatives.len(), 2);

    // Same answer, far fewer calls, when both plans actually run.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let sink = demo_sink(&c, 20).await;
        let (a, calls_written) = run_plan(&sink, written.logical()).await;
        let (b, calls_chosen) = run_plan(&sink, chosen.logical()).await;
        assert_eq!(a, b);
        assert!(!a.is_empty(), "the demo query should return rows");
        assert!(
            calls_written >= 10 * calls_chosen,
            "actual calls: written {calls_written}, chosen {calls_chosen}\n{text}"
        );
    });
}

#[test]
fn join_order_search_is_visibly_exponential() {
    let c = join_catalog();
    let cost = CostModel::default();
    // Seven relations: 7! × C(6) = 665,280 bushy orders, but the DP prices
    // only splits of subsets (3^7 - 2^8 + 1 = 1,932 at most).
    let sql = "SELECT p1.title FROM papers p1, papers p2, papers p3, papers p4, papers p5, papers p6, papers p7 \
WHERE p1.id = p2.id AND p2.id = p3.id AND p3.id = p4.id AND p4.id = p5.id AND p5.id = p6.id AND p6.id = p7.id \
AND relevant(p1.title) AND supports(p3.title, p7.title)";
    let cp = plan(&logical(sql, &c), &c, &cost);
    let ps = cp.plan_space.expect("plan space");
    assert_eq!(ps.relations, 7);
    assert_eq!(ps.orders, 665_280);
    assert!(ps.evaluated + ps.pruned <= 1_932, "{ps:?}");
    assert!(ps.evaluated + ps.pruned >= 100, "{ps:?}");
    assert!(
        explain(&cp).contains("665280 join orders"),
        "{}",
        explain(&cp)
    );
}

#[test]
fn cascade_routes_only_the_uncertain_band_to_the_oracle() {
    let mut c = join_catalog();
    c.set_proxy(
        "relevant",
        Some(callgebra_core::ProxySpec {
            function: "relevance_score".into(),
            low: 0.2,
            high: 0.8,
        }),
    );
    let mut cost = CostModel::default();
    cost.table_rows.insert("papers".into(), 100.0);
    let sql = "SELECT title FROM papers WHERE relevant(title)";
    let plain = plan_with(&logical(sql, &c), &c, &cost, &[]);
    let cp = plan(&logical(sql, &c), &c, &cost);
    let text = explain(&cp);
    assert!(cp.rules_applied.iter().any(|r| r == "cascade"), "{text}");
    // 100 proxy calls at a tenth of the price plus 60 oracle calls beat 100
    // oracle calls.
    assert!(text.contains("λ proxy"), "{text}");
    assert!(cp.total.dollars < plain.total.dollars, "{text}");
    assert!(
        (cp.total.calls - 160.0).abs() < 1e-6,
        "{}: {text}",
        cp.total.calls
    );
    // Executed: the oracle is asked only for the middle band ("tree" papers).
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let sink = demo_sink(&c, 20).await;
        let (plain_rows, plain_calls) = run_plan(&sink, plain.logical()).await;
        let (rows, calls) = run_plan(&sink, cp.logical()).await;
        assert_eq!(plain_calls, 20);
        assert_eq!(calls, 25, "20 proxy scores + 5 oracle calls for the band");
        // The cascade accepts high scorers without asking: same rows here
        // because the proxy agrees with the oracle on this data.
        assert_eq!(rows, plain_rows);
        assert_eq!(rows.len(), 5);
    });
}

#[test]
fn beam_limited_recursion_is_costed_per_frontier() {
    let c = catalog();
    let mut cost = CostModel::default();
    cost.table_rows.insert("possibilities".into(), 10.0);
    cost.branching = 4.0;
    cost.recursion_rounds = 4.0;
    let wide = "WITH RECURSIVE f(h, d) AS (SELECT candidate, 0 FROM possibilities UNION ALL \
SELECT e.item, f.d + 1 FROM f CROSS JOIN LATERAL expand(f.h, 4) AS e WHERE f.d < 3) SELECT h FROM f";
    let beam = "WITH RECURSIVE f(h, d) AS (SELECT candidate, 0 FROM possibilities UNION ALL \
SELECT e.item, f.d + 1 FROM f CROSS JOIN LATERAL expand(f.h, 4) AS e WHERE f.d < 3 ORDER BY e.item LIMIT 5) \
SELECT h FROM f";
    let wide_plan = plan(&logical(wide, &c), &c, &cost);
    let mut narrow = cost.clone();
    narrow.recursion_rounds = 3.0;
    let beam_plan = plan(&logical(beam, &c), &c, &narrow);
    let (wt, bt) = (explain(&wide_plan), explain(&beam_plan));
    assert!(bt.contains("beam 5"), "{bt}");
    // Unbounded: 10 + 40 + 160 expansions over four rounds;
    // beam: 10 + 5 + 5 over three rounds.
    assert!(
        wide_plan.total.calls > 5.0 * beam_plan.total.calls,
        "wide {} beam {}\n{wt}\n{bt}",
        wide_plan.total.calls,
        beam_plan.total.calls
    );
    assert!(
        (beam_plan.total.calls - 20.0).abs() < 1e-6,
        "{}: {bt}",
        beam_plan.total.calls
    );
}

#[test]
fn sampled_selectivity_changes_estimates_and_cheap_first_orders_join_conditions() {
    let c = join_catalog();
    let mut cost = CostModel::default();
    cost.table_rows.insert("papers".into(), 100.0);
    cost.table_rows.insert("claims".into(), 10.0);
    let sql =
        "SELECT p.title FROM papers p JOIN claims c ON relevant(p.title) AND c.paper_id = p.id";
    let cp = plan(&logical(sql, &c), &c, &cost);
    let text = explain(&cp);
    // The pure join key moves ahead of the call inside the join condition.
    let key = text.find("paper_id = id").expect(&text);
    let call = text.find("relevant(").expect(&text);
    assert!(key < call, "{text}");
    // 1000 pairs × 0.1 join selectivity = 100 calls.
    assert!(
        (cp.total.calls - 100.0).abs() < 1e-6,
        "{}: {text}",
        cp.total.calls
    );
    // With an observed pass rate the output estimate follows it.
    let rows_default = cp.total.rows;
    cost.call_selectivity.insert("relevant".into(), 0.05);
    let cp2 = plan(&logical(sql, &c), &c, &cost);
    assert!(
        cp2.total.rows < rows_default,
        "{} vs {}",
        cp2.total.rows,
        rows_default
    );
}
