//! The daemon over a Unix socket: handshake, subscribe with replay and
//! cursors, start and watch a run, submit SQL, query the trace, cancel.

use kleene_core::{RunId, SessionId};
use kleene_daemon::client::Client;
use kleene_daemon::server::Daemon;
use kleene_daemon::{ClientRequest, Cursor, ServerMessage};
use kleene_harness::testing::ScriptedProvider;
use kleene_harness::HarnessConfig;
use kleene_store::DuckDbStore;
use kleene_trace::TraceEvent;
use std::sync::Arc;
use std::time::Duration;

struct Live {
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Live {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        self.server.abort();
    }
}

async fn start(rules: Vec<(&str, &str)>) -> Live {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("d.sock");
    let store = DuckDbStore::in_memory().unwrap();
    let provider = Arc::new(ScriptedProvider::new(rules, "I do not know."));
    let cfg = HarnessConfig {
        workspace: dir.path().to_path_buf(),
        provider: Some(provider),
        max_turns: 4,
        ..HarnessConfig::default()
    };
    let daemon = Daemon::new(store, cfg).await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let sock = socket.clone();
    let server = tokio::spawn(async move {
        let _ = daemon
            .serve_until(&sock, async {
                let _ = stopped.await;
            })
            .await;
    });
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Live {
        socket,
        _dir: dir,
        stop: Some(stop),
        server,
    }
}

fn sql(s: &str) -> String {
    format!("```sql\n{s}\n```")
}

async fn next(client: &mut Client) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(10), client.recv())
        .await
        .expect("timed out waiting for the daemon")
        .unwrap()
        .expect("daemon closed the connection")
}

#[tokio::test]
async fn run_is_streamed_and_replayable() {
    let create = sql("CREATE TABLE t AS SELECT 3 AS x");
    let fin = sql("FINAL FROM (SELECT x FROM t)");
    let live = start(vec![("turn 1/", fin.as_str()), ("# Task", create.as_str())]).await;

    let mut watcher = Client::connect(&live.socket).await.unwrap();
    watcher
        .send(&ClientRequest::Subscribe { after: None })
        .await
        .unwrap();
    let mut starter = Client::connect(&live.socket).await.unwrap();
    let accepted = starter
        .call(&ClientRequest::StartRun {
            task: "Make three.".into(),
            workspace: String::new(),
            context: None,
            max_turns: None,
            max_depth: None,
            budget_calls: None,
            check: None,
            conversation: None,
        })
        .await
        .unwrap();
    let ServerMessage::RunAccepted { run } = accepted else {
        panic!("{accepted:?}")
    };

    // The watcher sees the whole story in order, with monotonic cursors.
    let mut seen = vec![];
    let mut last_cursor: Option<Cursor> = None;
    let mut finished = None;
    while finished.is_none() {
        match next(&mut watcher).await {
            ServerMessage::Event { cursor, traced } => {
                if let Some(l) = last_cursor {
                    assert!(cursor > l, "cursors must increase");
                }
                last_cursor = Some(cursor);
                seen.push(traced.event);
            }
            ServerMessage::RunFinished {
                run: r,
                outcome,
                answer,
                answer_columns,
                answer_rows,
            } => {
                assert_eq!(answer_columns, ["x"], "{answer_columns:?}");
                assert!(
                    answer_rows.iter().any(|r| r == &["3".to_string()]),
                    "{answer_rows:?}"
                );
                assert_eq!(r, run);
                finished = Some((outcome, answer));
            }
            _ => {}
        }
    }
    let (outcome, answer) = finished.unwrap();
    assert_eq!(outcome, "final");
    assert!(answer.unwrap().contains('3'));
    assert!(seen
        .iter()
        .any(|e| matches!(e, TraceEvent::RunStarted { run: r, .. } if *r == run)));
    assert!(seen
        .iter()
        .any(|e| matches!(e, TraceEvent::SessionFinished { outcome, .. } if outcome == "final")));
    let statements = seen
        .iter()
        .filter(|e| matches!(e, TraceEvent::StatementStarted { .. }))
        .count();
    assert_eq!(statements, 4, "two turns and two statements: {seen:?}");

    // A late client replays from a cursor and gets only what follows.
    let mut late = Client::connect(&live.socket).await.unwrap();
    let mid = Cursor {
        generation: late.generation,
        sequence: 2,
    };
    late.send(&ClientRequest::Subscribe { after: Some(mid) })
        .await
        .unwrap();
    let first = next(&mut late).await;
    match first {
        ServerMessage::Event { cursor, .. } => assert_eq!(cursor.sequence, 3),
        ServerMessage::RunAccepted { .. } | ServerMessage::RunFinished { .. } => {}
        other => panic!("{other:?}"),
    }
    // No live runs remain, and the trace is queryable.
    let runs = starter.call(&ClientRequest::ListRuns).await.unwrap();
    assert_eq!(runs, ServerMessage::Runs { runs: vec![] });
    let table = starter
        .call(&ClientRequest::Query {
            sql: "SELECT outcome, turns FROM trace_sessions".into(),
            tag: None,
        })
        .await
        .unwrap();
    let ServerMessage::Table { columns, rows, .. } = table else {
        panic!("{table:?}")
    };
    assert_eq!(columns, ["outcome", "turns"]);
    assert_eq!(rows, vec![vec!["final".to_string(), "2".to_string()]]);
}

#[tokio::test]
async fn submit_opens_interactive_sessions_and_errors_are_messages() {
    let live = start(vec![]).await;
    let mut c = Client::connect(&live.socket).await.unwrap();
    let session = SessionId::new();
    let reply = c
        .call(&ClientRequest::Submit {
            session,
            sql: "CREATE TABLE t AS SELECT 1 AS a; SELECT a + 1 AS b FROM t".into(),
        })
        .await
        .unwrap();
    let ServerMessage::Submitted {
        session: s,
        results,
    } = reply
    else {
        panic!("{reply:?}")
    };
    assert_eq!(s, session);
    assert_eq!(results.len(), 2);
    assert!(results[1].text.contains('2'), "{}", results[1].text);
    // The same session id keeps its state.
    let reply = c
        .call(&ClientRequest::Submit {
            session,
            sql: "SELECT COUNT(*) FROM t".into(),
        })
        .await
        .unwrap();
    assert!(matches!(reply, ServerMessage::Submitted { results, .. } if !results[0].is_error));
    let bad = c
        .call(&ClientRequest::CancelRun { run: RunId::new() })
        .await
        .unwrap();
    assert!(matches!(bad, ServerMessage::Error { .. }));
    let bad = c
        .call(&ClientRequest::Query {
            sql: "SELECT * FROM nope".into(),
            tag: None,
        })
        .await
        .unwrap();
    assert!(matches!(bad, ServerMessage::Error { .. }));
    // Detach closes cleanly.
    c.send(&ClientRequest::Detach).await.unwrap();
    assert!(c.recv().await.unwrap().is_none());
}

#[tokio::test]
async fn cancel_run_stops_it() {
    // A run that never finishes on its own: the model keeps peeking.
    let peek = sql("SELECT 1");
    let live = start(vec![("", peek.as_str())]).await;
    let mut c = Client::connect(&live.socket).await.unwrap();
    let accepted = c
        .call(&ClientRequest::StartRun {
            task: "Loop.".into(),
            workspace: String::new(),
            context: None,
            max_turns: Some(1000),
            max_depth: None,
            budget_calls: None,
            check: None,
            conversation: None,
        })
        .await
        .unwrap();
    let ServerMessage::RunAccepted { run } = accepted else {
        panic!("{accepted:?}")
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let runs = c.call(&ClientRequest::ListRuns).await.unwrap();
    assert_eq!(runs, ServerMessage::Runs { runs: vec![run] });
    let ok = c.call(&ClientRequest::CancelRun { run }).await.unwrap();
    assert!(matches!(ok, ServerMessage::Ok { .. }), "{ok:?}");
    let runs = c.call(&ClientRequest::ListRuns).await.unwrap();
    assert_eq!(runs, ServerMessage::Runs { runs: vec![] });
}
