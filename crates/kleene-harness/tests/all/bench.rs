//! Benchmarks over task packs: the Terminal-Bench-style pack with a solver
//! that drives the shell surface from SQL, the plain-agent baseline on the
//! same provider, learning versus frozen on a frozen generator stream, and
//! the report, curve and CSV.

use kleene_harness::learn::bench::Mode;
use kleene_harness::learn::packs::{terminal_pack, Pack};
use kleene_harness::learn::{Learn, LearnConfig};
use kleene_harness::testing::ScriptedProvider;
use kleene_harness::HarnessConfig;
use kleene_store::DuckDbStore;
use std::sync::Arc;

async fn learn(store: DuckDbStore, provider: Arc<ScriptedProvider>) -> Learn {
    let cfg = HarnessConfig {
        workspace: std::env::temp_dir(),
        provider: Some(provider),
        max_turns: 4,
        ..HarnessConfig::default()
    };
    Learn::new(
        store,
        cfg,
        LearnConfig {
            replay_sample: 0,
            ..LearnConfig::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn terminal_pack_runs_shell_tasks_from_sql_and_shell_oracles_judge_them() {
    let dir = tempfile::tempdir().unwrap();
    let pack = terminal_pack();
    pack.save(dir.path()).unwrap();
    let loaded = Pack::load(dir.path()).unwrap();
    assert_eq!(loaded.tasks.len(), 6);
    // A solver for two of the tasks: it creates the file with write_file
    // and counts lines with a shell call, then answers.
    let p = Arc::new(ScriptedProvider::new(
        vec![
            (
                "Create a file named hello.txt",
                "```sql\nCALL write_file('hello.txt', 'hello, world');\nFINAL FROM (SELECT true AS done)\n```",
            ),
            (
                "How many lines does it have",
                "```sql\nFINAL FROM (SELECT COUNT(*) AS lines FROM lines('data.txt'))\n```",
            ),
        ],
        "```sql\nFINAL FROM (SELECT 'no idea' AS answer)\n```",
    ));
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), p).await;
    let report = l
        .bench(dir.path(), &loaded, Mode::Frozen, Some(3))
        .await
        .unwrap();
    assert_eq!(report.rows.len(), 3, "{report:?}");
    let by_id = |id: &str| report.rows.iter().find(|r| r.task == id).unwrap();
    assert!(by_id("create-file").solved, "{:?}", by_id("create-file"));
    assert!(by_id("count-lines").solved, "{:?}", by_id("count-lines"));
    assert!(!by_id("rename-extension").solved);
    // Each task ran in its own fresh workspace: the shared temp dir has no hello.txt from us.
    let summary = l.bench_summary().await.unwrap().render_table(20);
    assert!(summary.contains("terminal"), "{summary}");
    assert!(summary.contains("frozen"), "{summary}");
    let csv = l.bench_csv().await.unwrap();
    assert_eq!(csv.lines().count(), 4, "{csv}");
    assert!(csv.starts_with("run,pack,mode,seq,task"));
    let curve = l.bench_curve(&report.run).await.unwrap();
    assert_eq!(curve.iter().filter(|(_, s)| *s).count(), 2);
    // The plots draw from the same tables: the curve and the cost-parity
    // chart have this run's points, and a chart with no data (this fixture
    // has no tracer, so no statement estimates; no ratings either) says so
    // instead of rendering nothing.
    let plots = l.bench_plots().await.unwrap();
    let names: Vec<&str> = plots.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "learning_curve.svg",
            "cost_parity.svg",
            "calls_vs_difficulty.svg",
            "estimate_accuracy.svg",
            "plan_space.svg"
        ]
    );
    let by_name = |n: &str| &plots.iter().find(|(m, _)| m == n).unwrap().1;
    assert!(
        by_name("learning_curve.svg").contains("<polyline"),
        "{}",
        by_name("learning_curve.svg")
    );
    assert!(by_name("cost_parity.svg").contains("terminal frozen"));
    assert!(by_name("estimate_accuracy.svg").contains("no data yet"));
    // A pack task is an attempt, so the difficulty plot has its points.
    assert!(
        by_name("calls_vs_difficulty.svg").contains("<circle"),
        "{}",
        by_name("calls_vs_difficulty.svg")
    );
    let attempts = store
        .query("SELECT solver, COUNT(*) FROM attempts GROUP BY solver")
        .await
        .unwrap();
    assert_eq!(attempts.rows[0][0].render(), "bench:frozen");
    assert_eq!(attempts.rows[0][1].render(), "3");
}

#[tokio::test]
async fn plain_agent_baseline_uses_tools_through_json_actions() {
    let dir = tempfile::tempdir().unwrap();
    let pack = terminal_pack();
    pack.save(dir.path()).unwrap();
    // The plain agent answers count-lines by calling shell, then finishing.
    let p = Arc::new(ScriptedProvider::new(
        vec![
            ("stdout", r#"{"final": [["5"]], "columns": ["lines"]}"#),
            (
                "How many lines does it have",
                r#"{"tool": "shell", "args": ["wc -l < data.txt"]}"#,
            ),
        ],
        r#"{"final": [["no idea"]], "columns": ["answer"]}"#,
    ));
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), p).await;
    let report = l
        .bench(dir.path(), &pack, Mode::Plain, Some(2))
        .await
        .unwrap();
    let count = report
        .rows
        .iter()
        .find(|r| r.task == "count-lines")
        .unwrap();
    assert!(count.solved, "{count:?}");
    assert_eq!(count.calls, 2, "one tool call, one final");
    let create = report
        .rows
        .iter()
        .find(|r| r.task == "create-file")
        .unwrap();
    assert!(!create.solved);
    let summary = l.bench_summary().await.unwrap().render_table(20);
    assert!(summary.contains("plain"), "{summary}");
}

#[tokio::test]
async fn learning_mode_adopts_a_playbook_and_frozen_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let pack = Pack::from_generator("puzzles", "puzzle", 3, 0.2, 5, &std::env::temp_dir()).unwrap();
    pack.save(dir.path()).unwrap();
    let mut rules = vec![];
    for k in 2..=8 {
        rules.push((
            format!("divisible by {k} (0 if none)"),
            format!("```sql\nFINAL FROM (SELECT COALESCE(SUM(CAST(text AS BIGINT)), 0) AS total FROM ctx WHERE CAST(text AS BIGINT) % {k} = 0)\n```"),
        ));
    }
    let rules: Vec<(&str, &str)> = rules
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let p = Arc::new(ScriptedProvider::new(
        rules,
        "```sql\nFINAL FROM (SELECT 0 AS total)\n```",
    ));
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), p).await;
    let frozen = l
        .bench(dir.path(), &pack, Mode::Frozen, None)
        .await
        .unwrap();
    assert!((frozen.accuracy() - 1.0).abs() < 1e-12, "{frozen:?}");
    assert!(l
        .playbook_for("puzzle_divisible_sum")
        .await
        .unwrap()
        .is_empty());
    let learning = l
        .bench(dir.path(), &pack, Mode::Learning, None)
        .await
        .unwrap();
    assert!((learning.accuracy() - 1.0).abs() < 1e-12);
    assert!(!l
        .playbook_for("puzzle_divisible_sum")
        .await
        .unwrap()
        .is_empty());
    let summary = l.bench_summary().await.unwrap();
    assert_eq!(summary.rows.len(), 2, "{}", summary.render_table(10));
    assert_eq!(learning.curve(3).last().copied(), Some(1.0));
}

/// A provider that is never reachable: every call fails before a token is
/// spent, the way a key without credit or a dead endpoint does.
struct DeadProvider;

#[async_trait::async_trait]
impl kleene_llm::Provider for DeadProvider {
    fn name(&self) -> &str {
        "dead"
    }

    fn capabilities(&self) -> kleene_llm::Capabilities {
        kleene_llm::Capabilities {
            streaming: false,
            tools: false,
            json_schema: true,
            prompt_cache: false,
            reasoning_control: false,
            cost_reported: true,
        }
    }

    async fn complete(
        &self,
        _req: kleene_llm::CompletionRequest,
    ) -> Result<kleene_llm::CompletionResponse, kleene_llm::ProviderError> {
        Err(kleene_llm::ProviderError::Other(
            "Your credit balance is too low to access the API".into(),
        ))
    }
}

#[tokio::test]
async fn a_run_stops_when_the_provider_never_answers_and_resumes_where_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let pack = terminal_pack();
    pack.save(dir.path()).unwrap();
    let loaded = Pack::load(dir.path()).unwrap();
    let store = DuckDbStore::in_memory().unwrap();
    // The first two tasks run against a working provider, the third against
    // a dead one: the run must stop there, not record the rest as failures.
    let working = Arc::new(ScriptedProvider::new(
        vec![],
        "```sql\nFINAL FROM (SELECT 'no idea' AS answer)\n```",
    ));
    let l = learn(store.clone(), working.clone()).await;
    let partial = l
        .bench(dir.path(), &loaded, Mode::Frozen, Some(2))
        .await
        .unwrap();
    assert_eq!(partial.rows.len(), 2);
    let run = partial.run.clone();

    let dead_cfg = HarnessConfig {
        workspace: std::env::temp_dir(),
        provider: Some(Arc::new(DeadProvider)),
        max_turns: 4,
        ..HarnessConfig::default()
    };
    let dead = Learn::new(
        store.clone(),
        dead_cfg,
        LearnConfig {
            replay_sample: 0,
            ..LearnConfig::default()
        },
    )
    .await
    .unwrap();
    let err = dead
        .bench_from(dir.path(), &loaded, Mode::Frozen, None, Some(&run))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("aborted at task 3"), "{err}");
    assert!(err.contains("credit balance"), "{err}");
    assert!(err.contains(&format!("--resume {run}")), "{err}");
    let evals = store
        .query(&format!("SELECT COUNT(*) FROM evals WHERE run = '{run}'"))
        .await
        .unwrap();
    assert_eq!(evals.rows[0][0].render(), "2", "the dead task left no row");
    let note = store
        .query(&format!("SELECT note FROM bench_runs WHERE run = '{run}'"))
        .await
        .unwrap();
    assert!(note.rows[0][0].render().contains("aborted at task 3"));

    // Resumed with a working provider, the run finishes with every task in
    // order and the earlier rows intact.
    let full = l
        .bench_from(dir.path(), &loaded, Mode::Frozen, None, Some(&run))
        .await
        .unwrap();
    assert_eq!(full.run, run);
    assert_eq!(full.rows.len(), 6);
    assert_eq!(
        full.rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        (0..6).collect::<Vec<_>>()
    );
    assert_eq!(working.calls(), 2 + 4);
    let note = store
        .query(&format!("SELECT note FROM bench_runs WHERE run = '{run}'"))
        .await
        .unwrap();
    assert_eq!(note.rows[0][0].render(), "NULL");

    // A different pack or mode cannot continue the run.
    let err = l
        .bench_from(dir.path(), &loaded, Mode::Plain, None, Some(&run))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not terminal in plain mode"), "{err}");
}
