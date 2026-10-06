//! Model calls, prompt-defined functions, CALL tools, memo, budgets, EXPLAIN.

use kleene_harness::testing::ScriptedProvider;
use kleene_harness::{Repl, ReplConfig};
use kleene_store::DuckDbStore;
use std::sync::Arc;

struct Fixture {
    repl: Repl,
    trace: kleene_store::DuckDbTraceSink,
    dir: tempfile::TempDir,
}

async fn repl(provider: Arc<ScriptedProvider>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = DuckDbStore::in_memory().unwrap();
    let trace = store.trace_sink();
    let tracer = Some(kleene_trace::Tracer::new(Arc::new(trace.clone())));
    let r = Repl::with_config(
        store,
        ReplConfig {
            workspace: dir.path().to_path_buf(),
            provider: Some(provider),
            tracer,
            web_search: None,
        },
    )
    .await
    .unwrap();
    Fixture {
        repl: r,
        trace,
        dir,
    }
}

fn text(out: &[kleene_harness::Rendered]) -> String {
    out.iter()
        .map(|r| r.text.clone())
        .collect::<Vec<_>>()
        .join("\n---\n")
}

#[tokio::test]
async fn verify_and_refute_pitch_query_with_explain_analyze() {
    let p = Arc::new(ScriptedProvider::new(
        vec![
            ("Is candidate 'bad idea'", r#"{"answer": false}"#),
            ("Is candidate", r#"{"answer": true}"#),
            (
                "refuted by 'this refutes gamma' -- candidate 'gamma'",
                r#"{"answer": true}"#,
            ),
            ("refuted by", r#"{"answer": false}"#),
        ],
        r#"{"answer": false}"#,
    ));
    let f = repl(p.clone()).await;
    let r = &f.repl;
    let out = r
        .submit(
            "CREATE TABLE possibilities AS SELECT * FROM (VALUES ('alpha'), ('bad idea'), ('gamma')) AS t(candidate); \
             CREATE TABLE counterexamples AS SELECT * FROM (VALUES ('this refutes gamma')) AS t(text)",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    let out = r
        .submit("CREATE FUNCTION verify(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is candidate ''{c}'' a good idea?' BATCH 1")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    let out = r
        .submit("CREATE FUNCTION refute(c TEXT, ce TEXT) RETURNS BOOLEAN AS PROMPT 'Is it refuted by ''{ce}'' -- candidate ''{c}''?' BATCH 1")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    let sql = "SELECT candidate FROM possibilities WHERE verify(candidate) AND NOT EXISTS (SELECT 1 FROM counterexamples ce WHERE refute(candidate, ce.text)) ORDER BY candidate";
    let out = r.submit(&format!("EXPLAIN {sql}")).await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("λ worker"), "{}", out[0].text);
    assert!(out[0].text.contains("semi-join"), "{}", out[0].text);
    assert!(out[0].text.contains("fragment: FO"), "{}", out[0].text);
    let out = r.submit(&format!("EXPLAIN ANALYZE {sql}")).await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("actual: 1 rows, 5 calls"),
        "{}",
        out[0].text
    );
    // 3 verify calls + refute on the two survivors (alpha, gamma) = 5 calls.
    assert_eq!(p.calls(), 5, "{}", out[0].text);
    let out = r.submit(sql).await;
    assert!(out[0].text.contains("alpha"), "{}", out[0].text);
    assert!(!out[0].text.contains("gamma"), "{}", out[0].text);
    // Second run is served from the memo: no new provider calls.
    assert_eq!(p.calls(), 5);
    assert!(
        out[0].text.contains("5 calls (5 from memo)"),
        "{}",
        out[0].text
    );
    // The trace recorded calls and statements.
    let store = r.sink().store().store().clone();
    f.trace.flush().await;
    let calls = store
        .query("SELECT COUNT(*) FROM trace_calls WHERE memo_hit")
        .await
        .unwrap();
    assert!(calls.rows[0][0].as_int().unwrap() >= 5, "{:?}", calls.rows);
    let stmts = store
        .query("SELECT COUNT(*) FROM trace_statements WHERE calls > 0")
        .await
        .unwrap();
    assert!(stmts.rows[0][0].as_int().unwrap() >= 1);
    // Every planned statement records its estimate beside the actuals, so
    // `bench plot` can draw estimated against actual calls.
    let planned = store
        .query(
            "SELECT COUNT(*) FROM trace_statements WHERE calls > 0 AND estimate LIKE '%\"calls\"%'",
        )
        .await
        .unwrap();
    assert!(
        planned.rows[0][0].as_int().unwrap() >= 1,
        "{:?}",
        planned.rows
    );
}

#[tokio::test]
async fn llm_functions_and_expand() {
    let p = Arc::new(ScriptedProvider::new(
        vec![
            ("colours", r#"["red", "green", "blue"]"#),
            ("yes or no", r#"{"answer": true}"#),
            ("as JSON", r#"{"n": 2}"#),
        ],
        "forty-two",
    ));
    let f = repl(p.clone()).await;
    let r = &f.repl;
    let out = r.submit("SELECT llm('what is six times seven') AS a, llm_bool('yes or no?') AS b, llm_json('give n as JSON', '{\"type\":\"object\"}') AS c").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("forty-two"), "{}", out[0].text);
    assert!(out[0].text.contains("true"), "{}", out[0].text);
    assert!(out[0].text.contains("{\"n\":2}"), "{}", out[0].text);
    let out = r.submit("SELECT e.item FROM (SELECT 1 AS x) s CROSS JOIN LATERAL expand('list three colours', 3) AS e ORDER BY 1").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("blue") && out[0].text.contains("red"),
        "{}",
        out[0].text
    );
    assert!(out[0].text.contains("3 rows"), "{}", out[0].text);
}

#[tokio::test]
async fn budgets_stop_statements() {
    let p = Arc::new(ScriptedProvider::new(vec![], "x"));
    let f = repl(p.clone()).await;
    let r = &f.repl;
    r.submit("SET budget.calls = 2").await;
    let out = r
        .submit("SELECT llm('q' || CAST(generate_series AS TEXT)) FROM generate_series(1, 5)")
        .await;
    assert!(out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("budget"), "{}", out[0].text);
    assert!(p.calls() <= 3, "{}", p.calls());
    let out = r.submit("SET budget.nope = 1").await;
    assert!(out[0].is_error);
}

#[tokio::test]
async fn call_tools_and_shell_functions() {
    let p = Arc::new(ScriptedProvider::new(vec![], "x"));
    let f = repl(p).await;
    let r = &f.repl;
    let dir = &f.dir;
    let out = r.submit("CALL write_file('notes/a.txt', 'hello')").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes/a.txt")).unwrap(),
        "hello"
    );
    let out = r.submit("SELECT text FROM read('notes/a.txt')").await;
    assert!(out[0].text.contains("hello"), "{}", out[0].text);
    let out = r.submit("CALL shell('echo ' || name) FROM (SELECT * FROM (VALUES ('one'), ('two')) AS t(name)) AS q").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("one") && out[0].text.contains("two"),
        "{}",
        out[0].text
    );
    assert!(out[0].text.contains("2 rows"), "{}", out[0].text);
    // Volatile tools are refused outside CALL.
    let out = r.submit("SELECT * FROM shell('ls')").await;
    assert!(out[0].is_error);
    assert!(out[0].text.contains("CALL"), "{}", out[0].text);
    // A shell-defined predicate: exit code decides.
    r.submit(
        "CREATE FUNCTION is_even(n BIGINT) RETURNS BOOLEAN AS SHELL 'test $(( {n} % 2 )) -eq 0'",
    )
    .await;
    let out = r.submit("CALL is_even(4)").await;
    assert!(
        out[0].is_error,
        "shell-defined functions are scalars, not CALL targets: {}",
        out[0].text
    );
    let out = r.submit("SELECT is_even(4) AS a, is_even(3) AS b").await;
    assert!(
        out[0].is_error,
        "VOLATILE scalar functions are refused in expressions: {}",
        out[0].text
    );
    let out = r.submit("CREATE OR REPLACE FUNCTION is_even(n BIGINT) RETURNS BOOLEAN AS SHELL 'test $(( {n} % 2 )) -eq 0' STABLE").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    let out = r.submit("SELECT is_even(4) AS a, is_even(3) AS b").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("true") && out[0].text.contains("false"),
        "{}",
        out[0].text
    );
}

#[tokio::test]
async fn scripts_mix_calls_and_queries() {
    let p = Arc::new(ScriptedProvider::new(vec![], "x"));
    let f = repl(p).await;
    let out = f
        .repl
        .submit(
            "CALL write_file('a.txt', 'hi; there'); -- a comment; with a semicolon\n\
             SELECT text FROM read('a.txt'); CREATE FUNCTION two() RETURNS BIGINT AS SQL (SELECT 2); SELECT 1;",
        )
        .await;
    assert_eq!(out.len(), 4, "{}", text(&out));
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    assert!(out[1].text.contains("hi; there"), "{}", out[1].text);
    let out = f.repl.submit("SELECT 1; SELECT nope; SELECT 2").await;
    assert_eq!(out.len(), 2, "{}", text(&out));
    assert!(out[1].is_error);
}

#[tokio::test]
async fn no_provider_gives_a_clear_error() {
    let store = DuckDbStore::in_memory().unwrap();
    let r = Repl::new(store).await.unwrap();
    let out = r.submit("SELECT llm('hi')").await;
    assert!(out[0].is_error);
    assert!(out[0].text.contains("provider"), "{}", out[0].text);
}

#[tokio::test]
async fn planner_cascade_refusal_and_sampled_selectivity_in_the_repl() {
    let p = Arc::new(ScriptedProvider::new(
        vec![
            ("Score", "0.5"),
            ("relevant", r#"{"answer": true}"#),
            ("ok?", r#"{"answer": true}"#),
        ],
        "x",
    ));
    let f = repl(p).await;
    let r = &f.repl;
    let out = r
        .submit(
            "CREATE TABLE t AS SELECT 'row ' || generate_series AS x FROM generate_series(1, 4); \
             CREATE FUNCTION score(t TEXT) RETURNS DOUBLE AS PROMPT 'Score {t} from 0 to 1' MODEL 'proxy' BATCH 1; \
             CREATE FUNCTION rel(t TEXT) RETURNS BOOLEAN AS PROMPT 'Is {t} relevant?' BATCH 1 PROXY score THRESHOLDS (0.2, 0.8); \
             CREATE FUNCTION ok(t TEXT) RETURNS BOOLEAN AS PROMPT 'Is {t} ok?' BATCH 1",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    // A proxy must exist and score.
    let bad = r
        .submit("CREATE FUNCTION rel2(t TEXT) RETURNS BOOLEAN AS PROMPT 'x {t}' PROXY nope")
        .await;
    assert!(
        bad[0].is_error && bad[0].text.contains("nope"),
        "{}",
        bad[0].text
    );

    // EXPLAIN shows the cascade; running it scores every row and asks the
    // oracle only for the band (every score is 0.5 here, so all four).
    let out = r.submit("EXPLAIN SELECT x FROM t WHERE rel(x)").await;
    assert!(out[0].text.contains("cascade"), "{}", out[0].text);
    assert!(out[0].text.contains("score("), "{}", out[0].text);
    let out = r.submit("SELECT x FROM t WHERE rel(x)").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("4 rows"), "{}", out[0].text);
    assert!(out[0].text.contains("8 calls"), "{}", out[0].text);

    // Sampled selectivity: before any call the filter keeps half the rows;
    // after four `true` answers the estimate follows the observed rate.
    let before = r.submit("EXPLAIN SELECT x FROM t WHERE ok(x)").await;
    assert!(before[0].text.contains("~2 rows"), "{}", before[0].text);
    let out = r.submit("SELECT x FROM t WHERE ok(x)").await;
    assert!(out[0].text.contains("4 rows"), "{}", out[0].text);
    let after = r.submit("EXPLAIN SELECT x FROM t WHERE ok(x)").await;
    assert!(after[0].text.contains("~4.0 rows"), "{}", after[0].text);

    // A statement the remaining budget cannot pay for is refused up front,
    // with the estimate, before any call is made.
    r.submit("SET budget.calls = 14").await;
    let out = r.submit("SELECT x FROM t WHERE rel(x) AND ok(x)").await;
    assert!(out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("refused"), "{}", out[0].text);
    assert!(out[0].text.contains("remaining budget"), "{}", out[0].text);
    assert!(out[0].text.contains("total:"), "{}", out[0].text);
    drop(f.dir);
}

#[tokio::test]
async fn batch_functions_answer_many_rows_in_one_call() {
    let p = Arc::new(ScriptedProvider::new(
        vec![
            ("each of the 3 items", r#"["fruit", "vegetable", "fruit"]"#),
            ("Classify pear", "fruit"),
        ],
        "unknown",
    ));
    let f = repl(p.clone()).await;
    let r = &f.repl;
    let out = r
        .submit(
            "CREATE TABLE t AS SELECT 'apple' AS x UNION ALL SELECT 'leek' UNION ALL SELECT 'apple' UNION ALL SELECT 'plum'; \
             CREATE FUNCTION kind(x TEXT) RETURNS TEXT AS PROMPT 'Classify {x} as fruit or vegetable.' BATCH 10",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    // EXPLAIN prices the batch: three distinct rows in one call.
    let out = r.submit("EXPLAIN SELECT x, kind(x) FROM t").await;
    assert!(out[0].text.contains("×10"), "{}", out[0].text);
    assert!(out[0].text.contains("~1 call"), "{}", out[0].text);
    let out = r.submit("SELECT x, kind(x) AS k FROM t ORDER BY x").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("vegetable"), "{}", out[0].text);
    assert_eq!(p.calls(), 1, "one batched call for three distinct rows");
    // A batch answer that covers nothing falls back to one call per tuple
    // (asking the same batch again would only hit the memo).
    let p2 = Arc::new(ScriptedProvider::new(vec![("Classify", "fruit")], "[]"));
    let f2 = repl(p2.clone()).await;
    let out = f2
        .repl
        .submit(
            "CREATE TABLE t AS SELECT 'apple' AS x UNION ALL SELECT 'pear'; \
             CREATE FUNCTION kind(x TEXT) RETURNS TEXT AS PROMPT 'Classify {x}.' BATCH 10; \
             SELECT kind(x) FROM t",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    assert_eq!(p2.calls(), 3, "one failed batch, then one call per tuple");
    // Without BATCH a prompt function batches 20 per call.
    let out = f2
        .repl
        .submit(
            "CREATE FUNCTION kind2(x TEXT) RETURNS TEXT AS PROMPT 'Sort {x}.'; \
             EXPLAIN SELECT kind2(x) FROM t",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{}", text(&out));
    assert!(out[1].text.contains("×20"), "{}", out[1].text);
    drop(f.dir);
    drop(f2.dir);
}
