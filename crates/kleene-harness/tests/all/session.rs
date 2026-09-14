//! The turn loop, delegation (`rlm`, `spawn`), budgets, depth, persistence
//! and resume, driven by a scripted provider that answers by matching the
//! last user message.

use kleene_core::Budget;
use kleene_harness::testing::ScriptedProvider;
use kleene_harness::{Harness, HarnessConfig, Outcome, RunReport};
use kleene_store::DuckDbStore;
use std::sync::Arc;

struct Fixture {
    harness: Arc<Harness>,
    store: DuckDbStore,
    trace: kleene_store::DuckDbTraceSink,
    provider: Arc<ScriptedProvider>,
    _dir: tempfile::TempDir,
}

async fn fixture(rules: Vec<(&str, &str)>, tweak: impl FnOnce(&mut HarnessConfig)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = DuckDbStore::in_memory().unwrap();
    let trace = store.trace_sink();
    let provider = Arc::new(ScriptedProvider::new(rules, "I am not sure what to do."));
    let mut cfg = HarnessConfig {
        workspace: dir.path().to_path_buf(),
        provider: Some(provider.clone()),
        tracer: Some(kleene_trace::Tracer::new(Arc::new(trace.clone()))),
        max_turns: 6,
        child_max_turns: 4,
        ..HarnessConfig::default()
    };
    tweak(&mut cfg);
    let harness = Harness::new(store.clone(), cfg).await.unwrap();
    Fixture {
        harness,
        store,
        trace,
        provider,
        _dir: dir,
    }
}

fn sql(s: &str) -> String {
    format!("```sql\n{s}\n```")
}

fn final_rows(report: &RunReport) -> Vec<Vec<kleene_core::Value>> {
    match &report.root.outcome {
        Outcome::Final { answer } => answer.rows.clone(),
        other => {
            let dump: Vec<String> = report
                .root
                .transcript
                .iter()
                .map(|t| format!("--- turn {} ---\n{}\n>>>\n{}", t.n, t.reply, t.feedback))
                .collect();
            panic!("expected FINAL, got {other:?}\n{}", dump.join("\n"))
        }
    }
}

async fn count(store: &DuckDbStore, sql: &str) -> i64 {
    store.query(sql).await.unwrap().rows[0][0].as_int().unwrap()
}

#[tokio::test]
async fn root_session_runs_sql_until_final() {
    let create = sql("CREATE TABLE nums AS SELECT generate_series AS n FROM generate_series(1, 4);\nSELECT COUNT(*) AS c FROM nums");
    let fin = sql("FINAL FROM (SELECT SUM(n) AS total FROM nums)");
    let f = fixture(
        vec![("turn 1/", fin.as_str()), ("# Task", create.as_str())],
        |_| {},
    )
    .await;
    let report = f
        .harness
        .run("Sum the numbers one to four.", None)
        .await
        .unwrap();
    let rows = final_rows(&report);
    assert_eq!(rows, vec![vec![kleene_core::Value::Int(10)]]);
    assert_eq!(report.root.turns, 2);
    assert_eq!(report.root.transcript.len(), 2);
    assert!(
        report.root.transcript[0].feedback.contains("c\n-\n4"),
        "{}",
        report.root.transcript[0].feedback
    );
    assert!(report.root.transcript[0].feedback.contains("turn 1/6"));
    assert_eq!(f.provider.calls(), 2);
    // Persisted and traced.
    f.trace.flush().await;
    let outcome = f
        .store
        .query("SELECT outcome, turns FROM trace_sessions")
        .await
        .unwrap();
    assert_eq!(outcome.rows[0][0].as_text(), Some("final"));
    assert_eq!(outcome.rows[0][1].as_int(), Some(2));
    let status = f
        .store
        .query("SELECT status, outcome FROM kleene_sessions")
        .await
        .unwrap();
    assert_eq!(status.rows[0][0].as_text(), Some("finished"));
    assert_eq!(status.rows[0][1].as_text(), Some("final"));
    // Turn calls are traced as pseudo-statements, so calls per session add up.
    assert_eq!(
        count(
            &f.store,
            "SELECT COUNT(*) FROM trace_statements WHERE sql LIKE '-- turn%'"
        )
        .await,
        2
    );
    assert_eq!(count(&f.store, "SELECT COUNT(*) FROM trace_calls").await, 2);
}

#[tokio::test]
async fn prose_replies_are_nudged_and_errors_fed_back() {
    let fin = sql("FINAL(42)");
    let bad = sql("SELECT * FROM nowhere");
    let f = fixture(
        vec![
            ("Statement failed", fin.as_str()),
            ("No SQL found", bad.as_str()),
            ("# Task", "Let me think about this first."),
        ],
        |_| {},
    )
    .await;
    let report = f.harness.run("Answer 42.", None).await.unwrap();
    assert_eq!(report.root.turns, 3);
    let t = &report.root.transcript;
    assert!(t[0].sql.is_none());
    assert!(
        t[0].feedback.starts_with("No SQL found in your reply."),
        "{}",
        t[0].feedback
    );
    assert!(t[1].results[0].is_error);
    assert!(
        t[1].feedback.contains("unknown table: nowhere"),
        "{}",
        t[1].feedback
    );
    assert!(t[2].results[0].is_final);
    assert_eq!(final_rows(&report)[0][0], kleene_core::Value::Int(42));
}

#[tokio::test]
async fn rlm_maps_children_over_context_partitions() {
    let map = sql("CREATE TABLE answers AS SELECT c.ordinal, r.answer, r.detail FROM ctx c CROSS JOIN LATERAL rlm('Name the Greek letter in this text.', c.text) r;\nSELECT COUNT(*) AS n FROM answers");
    let fin = sql("FINAL FROM (SELECT ordinal, answer FROM answers ORDER BY ordinal)");
    let child = sql("FINAL FROM (SELECT upper(text) AS letter, ordinal FROM ctx)");
    let f = fixture(
        vec![
            ("Name the Greek letter", child.as_str()),
            ("turn 1/", fin.as_str()),
            ("Summarise", map.as_str()),
        ],
        |_| {},
    )
    .await;
    let context = "alpha is first\n\nbeta is second\n\ngamma is third".to_string();
    let report = f
        .harness
        .run("Summarise the context.", Some(context))
        .await
        .unwrap();
    let rows = final_rows(&report);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        rows[0][1],
        kleene_core::Value::Text("ALPHA IS FIRST".into())
    );
    assert_eq!(
        rows[2][1],
        kleene_core::Value::Text("GAMMA IS THIRD".into())
    );
    // One root turn call, one FINAL turn call, three child turn calls.
    assert_eq!(f.provider.calls(), 5);
    // Child spending rolls up into the root.
    assert_eq!(report.root.usage.calls, 5);
    f.trace.flush().await;
    assert_eq!(
        count(
            &f.store,
            "SELECT COUNT(*) FROM trace_sessions WHERE depth = 1 AND parent IS NOT NULL"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &f.store,
            "SELECT COUNT(*) FROM trace_sessions WHERE outcome = 'final'"
        )
        .await,
        4
    );
    // Children keep their own ctx tables under a namespace the root does not see.
    let tables = f
        .store
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'cgs_%__ctx'",
        )
        .await
        .unwrap();
    assert_eq!(tables.len(), 3);
    let root_catalog = f.harness.store().clone();
    let _ = root_catalog;
    let detail = f
        .store
        .query("SELECT detail FROM answers ORDER BY ordinal")
        .await
        .unwrap();
    let d = detail.rows[0][0].render();
    assert!(d.contains("\"letter\"") && d.contains("ALPHA"), "{d}");
}

#[tokio::test]
async fn spawn_runs_declared_agents_with_restricted_tools() {
    let declare = sql("CREATE AGENT reviewer MODEL 'worker' EFFORT 'low' TOOLS (read) BUDGET (calls 5) PROMPT 'You review one claim.';\nCREATE TABLE r AS SELECT s.answer, s.detail, s.session FROM spawn('reviewer', 'Check the claim carefully.', 'the claim text') s;\nSELECT answer FROM r");
    let fin = sql("FINAL FROM (SELECT answer FROM r)");
    let child_try_shell = sql("SELECT * FROM shell('ls')");
    let child_final = sql("FINAL('refuted')");
    let f = fixture(
        vec![
            ("Statement failed", child_final.as_str()),
            ("Check the claim", child_try_shell.as_str()),
            ("turn 1/", fin.as_str()),
            ("# Task", declare.as_str()),
        ],
        |_| {},
    )
    .await;
    let report = f.harness.run("Review the claim.", None).await.unwrap();
    let rows = final_rows(&report);
    assert_eq!(rows, vec![vec![kleene_core::Value::Text("refuted".into())]]);
    // The child could not see `shell`: the catalog was restricted to `read`.
    f.trace.flush().await;
    let child_stmts = f
        .store
        .query("SELECT error FROM trace_statements WHERE sql LIKE 'SELECT * FROM shell%'")
        .await
        .unwrap();
    let err = child_stmts.rows[0][0].render();
    assert!(err.contains("shell"), "{err}");
    let roles = f
        .store
        .query("SELECT role FROM trace_sessions WHERE depth = 1")
        .await
        .unwrap();
    assert_eq!(roles.rows[0][0].as_text(), Some("reviewer"));

    // Unknown agents are refused with a hint.
    let bad = sql("SELECT * FROM spawn('nobody', 'x')");
    let fin2 = sql("FINAL(1)");
    let f = fixture(
        vec![
            ("Statement failed", fin2.as_str()),
            ("# Task", bad.as_str()),
        ],
        |_| {},
    )
    .await;
    let report = f.harness.run("Try an unknown agent.", None).await.unwrap();
    let t = &report.root.transcript[0];
    assert!(
        t.results[0].text.contains("no agent named nobody"),
        "{}",
        t.results[0].text
    );
}

#[tokio::test]
async fn depth_and_budgets_are_enforced() {
    // Depth 0: rlm is refused with a clear error.
    let try_rlm = sql("SELECT * FROM rlm('q', 'c')");
    let fin = sql("FINAL(0)");
    let f = fixture(
        vec![
            ("Statement failed", fin.as_str()),
            ("# Task", try_rlm.as_str()),
        ],
        |c| c.max_depth = 0,
    )
    .await;
    let report = f.harness.run("Delegate.", None).await.unwrap();
    let text = &report.root.transcript[0].results[0].text;
    assert!(text.contains("depth 1 exceeds the maximum 0"), "{text}");

    // A one-call budget: the first turn spends it, the second cannot start.
    let f = fixture(vec![("# Task", "SELECT 1")], |c| {
        c.budget = Budget {
            calls: Some(1),
            ..Budget::unbounded()
        }
    })
    .await;
    let report = f.harness.run("Spend.", None).await.unwrap();
    assert!(
        matches!(report.root.outcome, Outcome::BudgetExhausted { .. }),
        "{:?}",
        report.root.outcome
    );
    assert_eq!(report.root.turns, 2);
    assert_eq!(f.provider.calls(), 1);

    // A child's slice is bounded by the role budget and rolls up.
    let declare = sql("CREATE AGENT tiny BUDGET (calls 1) PROMPT 'x';\nCREATE TABLE r AS SELECT answer, detail FROM spawn('tiny', 'Do the tiny job.') s;\nSELECT answer, detail FROM r");
    let fin = sql("FINAL FROM (SELECT detail FROM r)");
    let f = fixture(
        vec![
            ("Do the tiny job", "SELECT 1"),
            ("turn 1/", fin.as_str()),
            ("# Task", declare.as_str()),
        ],
        |_| {},
    )
    .await;
    let report = f.harness.run("Spawn a tiny agent.", None).await.unwrap();
    let rows = final_rows(&report);
    let detail = rows[0][0].render();
    assert!(detail.contains("budget_exhausted"), "{detail}");
}

#[tokio::test]
async fn turn_cap_then_resume_continues_the_transcript() {
    let step = sql("CREATE TABLE t AS SELECT 7 AS x");
    let fin = sql("FINAL FROM (SELECT x FROM t)");
    let f = fixture(
        vec![("turn 1/", fin.as_str()), ("# Task", step.as_str())],
        |c| c.max_turns = 1,
    )
    .await;
    let report = f.harness.run("Make seven.", None).await.unwrap();
    assert_eq!(report.root.outcome, Outcome::TurnsExhausted);
    assert_eq!(report.root.turns, 1);
    let run = report.run;
    f.trace.flush().await;
    assert_eq!(
        f.store
            .query("SELECT outcome FROM kleene_sessions")
            .await
            .unwrap()
            .rows[0][0]
            .as_text(),
        Some("turns_exhausted")
    );
    // A fresh harness over the same store picks the run up where it stopped.
    let cfg = HarnessConfig {
        provider: Some(f.provider.clone()),
        max_turns: 3,
        ..f.harness.config().clone()
    };
    let again = Harness::new(f.store.clone(), cfg).await.unwrap();
    let resumed = again.resume(run).await.unwrap();
    assert_eq!(resumed.run, run);
    assert_eq!(final_rows(&resumed), vec![vec![kleene_core::Value::Int(7)]]);
    assert_eq!(resumed.root.turns, 2, "turn numbering continues");
    assert_eq!(
        resumed.root.transcript.len(),
        1,
        "only the new turns are reported"
    );
    // The resumed transcript remembered the earlier exchange (the feedback of
    // turn 1 is what the scripted provider matched).
    assert!(resumed.root.transcript[0].results[0].is_final);
    let err = again.resume(run).await.unwrap_err();
    assert!(err.to_string().contains("ended with final"), "{err}");
    let err = again.resume(kleene_core::RunId::new()).await.unwrap_err();
    assert!(err.to_string().contains("no root session"), "{err}");
}

#[tokio::test]
async fn no_provider_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let store = DuckDbStore::in_memory().unwrap();
    let harness = Harness::new(
        store,
        HarnessConfig {
            workspace: dir.path().to_path_buf(),
            ..HarnessConfig::default()
        },
    )
    .await
    .unwrap();
    let report = harness.run("Anything.", None).await.unwrap();
    match report.root.outcome {
        Outcome::Failed { error } => assert!(error.contains("provider"), "{error}"),
        other => panic!("{other:?}"),
    }
}

/// Collects what the observer hears, so the streamed reply can be compared
/// with the transcript.
#[derive(Default)]
struct Ears {
    deltas: std::sync::Mutex<Vec<(u32, String)>>,
    starts: std::sync::Mutex<Vec<(u32, bool)>>,
    finishes: std::sync::Mutex<Vec<(u32, bool, Option<String>)>>,
}

impl kleene_harness::Observer for Ears {
    fn call_started(
        &self,
        meta: &kleene_harness::SessionMeta,
        _call: kleene_core::CallId,
        _alias: &str,
        turn: bool,
    ) {
        self.starts.lock().unwrap().push((meta.depth, turn));
    }
    fn call_delta(
        &self,
        meta: &kleene_harness::SessionMeta,
        _call: kleene_core::CallId,
        text: &str,
    ) {
        self.deltas
            .lock()
            .unwrap()
            .push((meta.depth, text.to_string()));
    }
    fn call_finished(
        &self,
        meta: &kleene_harness::SessionMeta,
        _call: kleene_core::CallId,
        memo_hit: bool,
        error: Option<&str>,
    ) {
        self.finishes
            .lock()
            .unwrap()
            .push((meta.depth, memo_hit, error.map(str::to_string)));
    }
}

#[tokio::test]
async fn observer_hears_the_reply_stream_and_call_lifecycle() {
    let create = sql("CREATE TABLE nums AS SELECT generate_series AS n FROM generate_series(1, 4);\nSELECT COUNT(*) AS c FROM nums");
    let fin = sql("FINAL FROM (SELECT SUM(n) AS total FROM nums)");
    let ears = Arc::new(Ears::default());
    let hooked = ears.clone();
    let f = fixture(
        vec![("count the numbers", create.as_str()), ("c", fin.as_str())],
        move |cfg| cfg.observer = Some(hooked),
    )
    .await;
    let report = f.harness.run("count the numbers", None).await.unwrap();
    assert_eq!(final_rows(&report)[0][0].as_int(), Some(10));
    // Every turn's reply reached the observer as streamed text, in order,
    // and concatenates to exactly what the transcript recorded.
    let streamed: String = ears
        .deltas
        .lock()
        .unwrap()
        .iter()
        .map(|(_, t)| t.as_str())
        .collect();
    let transcript: String = report
        .root
        .transcript
        .iter()
        .map(|t| t.reply.as_str())
        .collect();
    assert_eq!(streamed, transcript);
    let starts = ears.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 2, "two turns, two turn calls: {starts:?}");
    assert!(starts.iter().all(|(depth, turn)| *depth == 0 && *turn));
    let finishes = ears.finishes.lock().unwrap().clone();
    assert_eq!(finishes.len(), 2);
    assert!(
        finishes.iter().all(|(_, memo, err)| !memo && err.is_none()),
        "{finishes:?}"
    );
    assert_eq!(f.provider.calls(), 2);
}
