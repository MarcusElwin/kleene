//! REPL behaviour over an in-memory store.

use kleene_harness::Repl;
use kleene_store::DuckDbStore;

#[tokio::test]
async fn later_statements_see_tables_created_earlier() {
    let repl = Repl::new(DuckDbStore::in_memory().unwrap()).await.unwrap();
    let out = repl
        .submit("CREATE TABLE t AS SELECT generate_series AS n FROM generate_series(1, 3); SELECT SUM(n) AS s FROM t")
        .await;
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[1].text.contains("6"), "{}", out[1].text);
    // A fresh submission sees the table too, and errors carry hints.
    let out = repl.submit("SELECT nn FROM t").await;
    assert!(out[0].is_error);
    assert!(out[0].text.contains("hint"), "{}", out[0].text);
    let out = repl
        .submit("FINAL FROM (SELECT n FROM t WHERE n = 2)")
        .await;
    assert!(out[0].is_final);
}

#[tokio::test]
async fn execution_stops_at_the_first_error() {
    let repl = Repl::new(DuckDbStore::in_memory().unwrap()).await.unwrap();
    let out = repl
        .submit("SELECT 1 AS a; SELECT * FROM missing; SELECT 2 AS b")
        .await;
    assert_eq!(out.len(), 2);
    assert!(out[1].is_error);
}

/// A `FINAL` (or any CallSQL addition the parser does not know) is planned
/// when its turn comes, so it reads the tables the same reply created.
#[tokio::test]
async fn a_final_in_the_same_reply_reads_the_tables_that_reply_created() {
    let repl = Repl::new(DuckDbStore::in_memory().unwrap()).await.unwrap();
    let out = repl
        .submit("CREATE TABLE t AS SELECT generate_series AS n FROM generate_series(1, 3); CREATE TABLE total AS SELECT SUM(n) AS total FROM t; FINAL FROM (SELECT total FROM total)")
        .await;
    assert_eq!(out.len(), 3, "{out:?}");
    assert!(out[2].is_final, "{}", out[2].text);
    assert!(out[2].text.contains("6"), "{}", out[2].text);
    // A FINAL that is wrong is still reported in place, after what ran.
    let out = repl
        .submit("CREATE TABLE u AS SELECT 1 AS x; FINAL FROM (SELECT y FROM u)")
        .await;
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(!out[0].is_error);
    assert!(out[1].is_error, "{}", out[1].text);
}

/// The planner's dollar estimates follow the rates of the model actually
/// answering: once the trace holds a few priced calls, the cost model learns
/// the rate from them instead of the default rate card, so a cheap model is
/// not refused a statement the budget would easily cover.
#[tokio::test]
async fn token_rates_are_learned_from_the_traced_calls() {
    let repl = Repl::new(DuckDbStore::in_memory().unwrap()).await.unwrap();
    let before = repl.cost_model().await;
    assert_eq!(before.prices_learned, 0);
    let store = repl.sink().store().store().clone();
    // Twenty calls at $0.10 per M input and $0.50 per M output tokens, the
    // rate card of a cheap model; one memo hit and one failed call are not
    // priced and must not count.
    for i in 0..20 {
        store
            .execute(&format!(
                "INSERT INTO trace_calls (call, statement, alias, model, fingerprint, started_at, finished_at, input_tokens, output_tokens, cache_read_tokens, cost_usd, memo_hit, error) VALUES ('c{i}', 's', 'worker', 'cheap', 'f{i}', now(), now(), 10000, 1000, 0, 0.0015, false, NULL)"
            ))
            .await
            .unwrap();
    }
    store
        .execute("INSERT INTO trace_calls (call, statement, alias, model, fingerprint, started_at, finished_at, input_tokens, output_tokens, cache_read_tokens, cost_usd, memo_hit, error) VALUES ('memo', 's', 'worker', 'cheap', 'f0', now(), now(), 10000, 1000, 0, 0.0, true, NULL), ('failed', 's', 'worker', 'cheap', 'g', now(), now(), 0, 0, 0, 0.0, false, 'timeout')")
        .await
        .unwrap();
    let after = repl.cost_model().await;
    assert_eq!(after.prices_learned, 20);
    assert!(
        (after.usd_per_input_token - 1e-7).abs() < 1e-12,
        "{}",
        after.usd_per_input_token
    );
    assert!(
        (after.usd_per_output_token - 5e-7).abs() < 1e-12,
        "{}",
        after.usd_per_output_token
    );
    // Twenty times cheaper than the default rate card, children included.
    let ratio = before.child_session().dollars / after.child_session().dollars;
    assert!((ratio - 20.0).abs() < 1e-6, "{ratio}");
}
