//! The continual loop end to end over a scripted provider: generated tasks
//! with code oracles, curriculum and ratings, playbook candidates gated by
//! replay and revertable, the morning report, and resume by restart.

use callgebra_harness::learn::verify::Verify;
use callgebra_harness::learn::{Learn, LearnConfig, RunLimits};
use callgebra_harness::testing::ScriptedProvider;
use callgebra_harness::HarnessConfig;
use callgebra_store::DuckDbStore;
use std::sync::Arc;

/// A solver that always answers the divisible-sum puzzle correctly with one
/// SQL statement (it reads k from the task text), and answers nothing useful
/// for anything else.
fn puzzle_solver() -> Arc<ScriptedProvider> {
    let mut rules = vec![];
    for k in 2..=8 {
        rules.push((
            format!("divisible by {k} (0 if none)"),
            format!(
                "```sql\nFINAL FROM (SELECT COALESCE(SUM(CAST(text AS BIGINT)), 0) AS total FROM ctx WHERE CAST(text AS BIGINT) % {k} = 0)\n```"
            ),
        ));
    }
    let rules: Vec<(&str, &str)> = rules
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    Arc::new(ScriptedProvider::new(
        rules,
        "```sql\nFINAL FROM (SELECT 'no idea' AS answer)\n```",
    ))
}

async fn learn(store: DuckDbStore, provider: Arc<ScriptedProvider>, replay: usize) -> Learn {
    let dir = std::env::temp_dir();
    let cfg = HarnessConfig {
        workspace: dir,
        provider: Some(provider),
        max_turns: 3,
        ..HarnessConfig::default()
    };
    Learn::new(
        store,
        cfg,
        LearnConfig {
            replay_sample: replay,
            ..LearnConfig::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn loop_solves_generated_puzzles_learns_a_playbook_and_reports() {
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), puzzle_solver(), 2).await;
    let ids = l.generate("puzzle", 3).await.unwrap();
    assert_eq!(ids.len(), 3);
    let (rating0, _) = l.solver_rating().await.unwrap();

    let limits = RunLimits {
        max_tasks: 3,
        generators: vec!["puzzle".into()],
        queue_depth: 0,
        ..RunLimits::default()
    };
    let reports = l.run(&limits).await.unwrap();
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert!(reports.iter().all(|r| r.solved), "{reports:?}");
    // The first solve recorded a playbook candidate; the gate replayed and
    // adopted it (same solver, same answers, no extra cost).
    assert_eq!(reports[0].playbook_candidate, Some(1));
    assert_eq!(reports[0].adopted, Some(true), "{:?}", reports[0]);
    let pb = l.playbook_for("puzzle_divisible_sum").await.unwrap();
    assert!(!pb.is_empty());
    assert!(pb[0].sql.contains("FINAL FROM"), "{}", pb[0].sql);
    // Later attempts were shown the playbook and counted as wins.
    assert!(pb.iter().any(|p| p.tries >= 2 && p.wins >= 2), "{pb:?}");

    // Ratings moved: the solver up, the tasks down; the generator's dial
    // stepped up on a high solve rate.
    let (rating1, games) = l.solver_rating().await.unwrap();
    assert!(rating1 > rating0, "{rating0} -> {rating1}");
    assert_eq!(games, 3);
    let dial = l.dial("puzzle").await.unwrap();
    assert!(dial > 0.3, "dial {dial}");

    // The morning report and the board work.
    let report = l.report().await.unwrap();
    assert_eq!(report.rows.len(), 1, "{}", report.render_table(20));
    let text = report.render_table(20);
    assert!(text.contains("puzzle"), "{text}");
    let board = l.board().await.unwrap().render_table(20);
    assert!(board.contains("puzzle"), "{board}");
    let plan_query = store
        .query("SELECT generator, difficulty, AVG(solved), AVG(calls), AVG(depth) FROM trace_tasks GROUP BY 1, 2")
        .await
        .unwrap();
    assert_eq!(plan_query.rows.len(), 1);

    // Revert withdraws that version; later adopted versions stay.
    l.revert(1).await.unwrap();
    let pb = l.playbook_for("puzzle_divisible_sum").await.unwrap();
    assert!(pb.iter().all(|p| p.version != 1), "{pb:?}");
    assert!(l.revert(99).await.is_err());
    let ledger = l.playbook().await.unwrap().render_table(20);
    assert!(ledger.contains("reverted by hand"), "{ledger}");

    // Restart over the same store: state is all in tables, so the new loop
    // sees the same ratings and generator dial.
    let l2 = learn(store.clone(), puzzle_solver(), 2).await;
    let (rating2, games2) = l2.solver_rating().await.unwrap();
    assert_eq!((rating2, games2), (rating1, games));
    assert_eq!(l2.dial("puzzle").await.unwrap(), dial);
}

#[tokio::test]
async fn failures_lower_the_dial_and_user_tasks_wait_for_review() {
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), puzzle_solver(), 0).await;
    // The solver cannot do graphs: every attempt fails the exact oracle.
    l.generate("graph", 2).await.unwrap();
    let limits = RunLimits {
        max_tasks: 2,
        generators: vec!["graph".into()],
        queue_depth: 0,
        ..RunLimits::default()
    };
    let reports = l.run(&limits).await.unwrap();
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|r| !r.solved), "{reports:?}");
    assert!(reports.iter().all(|r| r.playbook_candidate.is_none()));
    assert!(l.dial("graph").await.unwrap() < 0.3);
    let (rating, _) = l.solver_rating().await.unwrap();
    assert!(rating < 1000.0);

    // A user task without an oracle lands in needs_review; one with an
    // exact oracle is judged.
    let id = l
        .add_task("question", "What is the answer?", None, None)
        .await
        .unwrap();
    let r = l.step(Some(&id), &[]).await.unwrap().unwrap();
    assert!(!r.solved);
    assert_eq!(l.task(&id).await.unwrap().unwrap().status, "needs_review");
    let id2 = l
        .add_task(
            "question",
            "Say no idea.",
            None,
            Some(Verify::Exact {
                rows: vec![vec!["no idea".into()]],
            }),
        )
        .await
        .unwrap();
    let r = l.step(Some(&id2), &[]).await.unwrap().unwrap();
    assert!(r.solved, "{r:?}");
    assert_eq!(l.task(&id2).await.unwrap().unwrap().status, "solved");

    // Nothing pending for an unknown generator; unknown generators are refused.
    assert!(l.step(None, &["sat3".into()]).await.unwrap().is_none());
    assert!(l.generate("nope", 1).await.is_err());
}

#[tokio::test]
async fn refill_keeps_queues_fed_and_curriculum_prefers_even_odds() {
    let store = DuckDbStore::in_memory().unwrap();
    let l = learn(store.clone(), puzzle_solver(), 0).await;
    let made = l
        .refill(&["puzzle".into(), "sat3".into()], 2)
        .await
        .unwrap();
    assert_eq!(made, 4);
    let made = l
        .refill(&["puzzle".into(), "sat3".into()], 2)
        .await
        .unwrap();
    assert_eq!(made, 0, "already full");
    // The generator prior for puzzles is easier than for 3-SAT at the same
    // dial, so a baseline solver is pointed at the puzzle nearest 0.5.
    let next = l.next_task(&[]).await.unwrap().unwrap();
    assert!(next.difficulty <= 1100.0, "{next:?}");
    let all = store
        .query("SELECT generator, COUNT(*) FROM tasks GROUP BY 1 ORDER BY 1")
        .await
        .unwrap();
    assert_eq!(all.rows.len(), 2);
}
