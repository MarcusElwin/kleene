//! Typed decisions (the `jev` alias): the `jev_*` functions, prompt-defined
//! functions on `MODEL 'jev'`, cascades with a Jev proxy, memo and trace.

use kleene_harness::testing::{ScriptedDecisions, ScriptedProvider};
use kleene_harness::{Repl, ReplConfig};
use kleene_store::DuckDbStore;
use std::sync::Arc;

struct Fixture {
    repl: Repl,
    store: DuckDbStore,
    trace: kleene_store::DuckDbTraceSink,
    decisions: Arc<ScriptedDecisions>,
    _dir: tempfile::TempDir,
}

async fn repl(decisions: Option<Arc<ScriptedDecisions>>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = DuckDbStore::in_memory().unwrap();
    let trace = store.trace_sink();
    let tracer = Some(kleene_trace::Tracer::new(Arc::new(trace.clone())));
    let decisions = decisions.unwrap_or_else(|| Arc::new(ScriptedDecisions::new(vec![], "0.5")));
    let repl = Repl::with_config(
        store.clone(),
        ReplConfig {
            workspace: dir.path().to_path_buf(),
            provider: Some(Arc::new(ScriptedProvider::new(
                vec![],
                r#"{"answer": true}"#,
            ))),
            decisions: Some(decisions.clone()),
            tracer,
            web_search: None,
        },
    )
    .await
    .unwrap();
    Fixture {
        repl,
        store,
        trace,
        decisions,
        _dir: dir,
    }
}

fn scripted() -> Arc<ScriptedDecisions> {
    Arc::new(ScriptedDecisions::new(
        vec![
            ("charged twice", "0.98"),
            ("tone", "angry"),
            ("severity", "1.3"),
            ("gamma", "0.9"),
            ("delta", "0.1"),
        ],
        "0.5",
    ))
}

async fn one(store: &DuckDbStore, sql: &str) -> kleene_core::Value {
    store.query(sql).await.unwrap().rows[0][0].clone()
}

#[tokio::test]
async fn scalar_decisions_answer_typed_and_go_through_memo_budget_and_trace() {
    let f = repl(Some(scripted())).await;
    let r = &f.repl;
    let out = r
        .submit(
            "SELECT jev_noul('I was charged twice. Fix this NOW.', 'Is this about billing?') AS p",
        )
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("0.98"), "{}", out[0].text);

    let out = r
        .submit("SELECT jev_choice('I was charged twice.', 'What is the tone?', '[\"calm\", \"angry\"]') AS tone")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("angry"), "{}", out[0].text);

    let out = r
        .submit("SELECT jev_score('the API is down', 'How severe is this? (severity)', 'minor, major, critical') AS s")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("1.3"), "{}", out[0].text);

    // The table form: every label with its probability, best first.
    let out = r
        .submit("SELECT label, probability FROM jev_choices('I was charged twice.', 'What is the tone?', 'calm, angry')")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    let angry = out[0].text.find("angry").unwrap();
    let calm = out[0].text.find("calm").unwrap();
    assert!(angry < calm, "most probable first: {}", out[0].text);

    // Three real decisions so far: jev_choices asked the same question with
    // the same labels as jev_choice, so it was a memo hit. Asking again is
    // another.
    assert_eq!(f.decisions.calls(), 3);
    assert_eq!(r.sink().memo_hits().await, 1);
    let out = r
        .submit(
            "SELECT jev_noul('I was charged twice. Fix this NOW.', 'Is this about billing?') AS p",
        )
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert_eq!(f.decisions.calls(), 3);
    assert_eq!(r.sink().memo_hits().await, 2);

    // Every decision is a trace row under the jev alias, memo hits included.
    f.trace.flush().await;
    let n = one(
        &f.store,
        "SELECT COUNT(*) FROM trace_calls WHERE alias = 'jev'",
    )
    .await;
    assert_eq!(n.as_int(), Some(5));
    let usage = r.sink().usage().await;
    assert_eq!(usage.calls, 3, "a memo hit takes no call slot");
    assert!(usage.dollars > 0.0, "decisions are charged");
}

#[tokio::test]
async fn nulls_bad_labels_and_a_missing_provider_are_clear() {
    let f = repl(Some(scripted())).await;
    let r = &f.repl;
    let out = r.submit("SELECT jev_noul(NULL, 'q') AS p").await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("NULL"), "{}", out[0].text);
    let out = r.submit("SELECT jev_choice('s', 'q', '') AS c").await;
    assert!(out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("at least one label"),
        "{}",
        out[0].text
    );
    assert_eq!(f.decisions.calls(), 0, "nothing reached the provider");

    let dir = tempfile::tempdir().unwrap();
    let bare = Repl::with_config(
        DuckDbStore::in_memory().unwrap(),
        ReplConfig {
            workspace: dir.path().to_path_buf(),
            provider: None,
            decisions: None,
            tracer: None,
            web_search: None,
        },
    )
    .await
    .unwrap();
    let out = bare.submit("SELECT jev_noul('s', 'q') AS p").await;
    assert!(out[0].is_error, "{}", out[0].text);
    assert!(
        out[0].text.contains("no decision provider") && out[0].text.contains("TYPESAFE_API_KEY"),
        "{}",
        out[0].text
    );
}

#[tokio::test]
async fn a_prompt_function_on_the_jev_alias_is_a_noul_and_a_ready_proxy() {
    let f = repl(Some(scripted())).await;
    let r = &f.repl;
    let out = r
        .submit(
            "CREATE FUNCTION plausible(c TEXT) RETURNS DOUBLE AS PROMPT 'Is {c} a plausible idea?' MODEL 'jev'; \
             CREATE FUNCTION likely(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is {c} a plausible idea?' MODEL 'jev'; \
             CREATE FUNCTION named(c TEXT) RETURNS TEXT AS PROMPT 'Name {c}' MODEL 'jev'",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{:?}", out);
    let out = r
        .submit("SELECT plausible('gamma') AS p, likely('gamma') AS b, likely('delta') AS d")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("0.9"), "{}", out[0].text);
    assert!(out[0].text.contains("true"), "{}", out[0].text);
    assert!(out[0].text.contains("false"), "{}", out[0].text);
    let out = r.submit("SELECT named('gamma') AS n").await;
    assert!(out[0].is_error, "{}", out[0].text);
    assert!(out[0].text.contains("jev_choice"), "{}", out[0].text);

    // The DOUBLE form is a proxy: the planner cascades the oracle behind it
    // and the band between the thresholds is all the text model sees.
    let out = r
        .submit(
            "CREATE TABLE ideas AS SELECT * FROM (VALUES ('gamma two'), ('delta two'), ('epsilon')) AS v(idea); \
             CREATE FUNCTION good(c TEXT) RETURNS BOOLEAN AS PROMPT 'Is {c} good?' PROXY plausible THRESHOLDS (0.2, 0.8)",
        )
        .await;
    assert!(out.iter().all(|o| !o.is_error), "{:?}", out);
    let out = r
        .submit("EXPLAIN SELECT idea FROM ideas WHERE good(idea)")
        .await;
    assert!(out[0].text.contains("cascade"), "{}", out[0].text);
    assert!(out[0].text.contains("plausible("), "{}", out[0].text);
    let before = f.decisions.calls();
    let out = r
        .submit("SELECT idea FROM ideas WHERE good(idea) ORDER BY idea")
        .await;
    assert!(!out[0].is_error, "{}", out[0].text);
    assert_eq!(
        f.decisions.calls(),
        before + 3,
        "every row is scored by Jev"
    );
    // gamma two (0.9) passes without the oracle, delta two (0.1) is dropped,
    // epsilon (0.5) is in the band and the scripted text model says yes.
    assert!(out[0].text.contains("gamma two"), "{}", out[0].text);
    assert!(out[0].text.contains("epsilon"), "{}", out[0].text);
    assert!(!out[0].text.contains("delta"), "{}", out[0].text);
}

#[tokio::test]
async fn the_catalog_shows_decisions_to_the_model() {
    let f = repl(None).await;
    let cat = f.repl.catalog();
    let cat = cat.read().await;
    for name in ["jev_noul", "jev_choice", "jev_score", "jev_choices"] {
        let def = cat
            .function(name)
            .unwrap_or_else(|| panic!("{name} in catalog"));
        assert!(def.description.contains("jev_"), "{name}");
    }
}
