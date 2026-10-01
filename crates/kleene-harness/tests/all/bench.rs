//! Benchmarks over task packs: the Terminal-Bench-style pack with a solver
//! that drives the shell surface from SQL, the plain-agent baseline on the
//! same provider, learning versus frozen on a frozen generator stream, and
//! the report, curve and CSV.

use kleene_harness::learn::bench::{BenchOptions, Mode};
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
    assert!(by_name("calls_vs_difficulty.svg").contains("no data yet"));
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

#[tokio::test]
async fn a_sampled_run_exports_outputs_in_the_lab_results_layout() {
    let dir = tempfile::tempdir().unwrap();
    let pack = terminal_pack();
    pack.save(dir.path()).unwrap();
    // The plain agent writes a deliverable under output/ for every task
    // (through write_file, a .docx built from the Markdown), then finishes.
    let p = Arc::new(ScriptedProvider::new(
        vec![(
            "Create a file named hello.txt",
            r##"{"tool": "write_file", "args": ["output/memo.docx", "# Memo. Done."]}"##,
        )],
        r#"{"final": [["done"]], "columns": ["answer"]}"#,
    ));
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), p).await;
    let out = tempfile::tempdir().unwrap();
    let report = l
        .bench_with(
            dir.path(),
            &pack,
            Mode::Plain,
            BenchOptions {
                limit: None,
                sample: Some((2, 3)),
                outputs: Some(out.path().to_path_buf()),
            },
        )
        .await
        .unwrap();
    assert_eq!(report.rows.len(), 2);
    // The sample keeps pack order and the same seed gives the same tasks.
    let ids: Vec<&str> = pack.tasks.iter().map(|t| t.id.as_str()).collect();
    let pos: Vec<usize> = report
        .rows
        .iter()
        .map(|r| ids.iter().position(|i| *i == r.task).unwrap())
        .collect();
    assert!(pos[0] < pos[1], "{pos:?}");
    assert_eq!(
        report
            .rows
            .iter()
            .map(|r| r.task.clone())
            .collect::<Vec<_>>(),
        pack.sampled(2, 3)
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
    );
    for r in &report.rows {
        let lab_run = r.exported.as_deref().unwrap();
        assert_eq!(lab_run, format!("{}/kleene-plain/{}", r.task, report.run));
        let d = out.path().join(lab_run);
        assert!(d.join("output").is_dir());
        assert!(d.join("config.json").is_file());
        let metrics: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join("metrics.json")).unwrap())
                .unwrap();
        assert_eq!(metrics["task"], r.task.as_str());
        assert_eq!(metrics["run_id"], lab_run);
        if r.task == "create-file" {
            assert!(d.join("output/memo.docx").is_file(), "deliverable exported");
        }
    }
}
