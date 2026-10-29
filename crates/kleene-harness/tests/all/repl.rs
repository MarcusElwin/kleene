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
