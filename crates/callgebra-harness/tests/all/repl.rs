//! REPL behaviour over an in-memory store.

use callgebra_harness::Repl;
use callgebra_store::DuckDbStore;

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
