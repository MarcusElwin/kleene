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
    // verify: 40 calls; refute: 40 outer rows × 5 counterexamples = 200 calls.
    assert!(
        (cp.total.calls - 240.0).abs() < 1e-9,
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
