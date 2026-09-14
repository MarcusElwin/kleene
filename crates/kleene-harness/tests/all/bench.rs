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
