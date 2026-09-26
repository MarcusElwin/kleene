//! The client-side picture of the engine, rebuilt from the daemon's event
//! stream: runs, the session tree, statements, calls and totals. Pure data,
//! so it is testable without a terminal.

use kleene_core::{CallId, RunId, SessionId, StatementId};
use kleene_daemon::{Cursor, ServerMessage};
use kleene_trace::TraceEvent;
use std::collections::HashMap;
use std::time::Duration;

/// A model call as the tree shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct CallNode {
    /// Id.
    pub id: CallId,
    /// Model alias.
    pub alias: String,
    /// Done.
    pub finished: bool,
    /// Served from the memo.
    pub memo_hit: bool,
    /// Tokens in + out.
    pub tokens: u64,
    /// Dollars.
    pub dollars: f64,
    /// Error text.
    pub error: Option<String>,
    /// Streamed text so far.
    pub streaming: String,
}

/// A statement as the tree shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatementNode {
    /// Id.
    pub id: StatementId,
    /// SQL text.
    pub sql: String,
    /// Done.
    pub finished: bool,
    /// Rows produced.
    pub rows: u64,
    /// Calls made (from the finish event).
    pub calls: u64,
    /// Tokens used.
    pub tokens: u64,
    /// Dollars spent.
    pub dollars: f64,
    /// Wall time.
    pub elapsed: Duration,
    /// Error text.
    pub error: Option<String>,
    /// `EXPLAIN` rendering, if planned.
    pub explain: Option<String>,
    /// Calls, in order.
    pub call_ids: Vec<CallId>,
    /// Tool calls: `tool(args) -> bytes or error`.
    pub tool_calls: Vec<String>,
    /// Recursion rounds: `(cte, round, delta, total)`.
    pub rounds: Vec<(String, u32, u64, u64)>,
}

/// One completed turn of a session, as the daemon reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRecord {
    /// 1-based turn number.
    pub n: u32,
    /// The model's reply, verbatim.
    pub reply: String,
    /// The SQL extracted from it, if any.
    pub sql: Option<String>,
    /// What each statement rendered to.
    pub results: Vec<kleene_daemon::StatementOutput>,
}

/// A session as the stream shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionNode {
    /// Id.
    pub id: SessionId,
    /// Run.
    pub run: RunId,
    /// Parent.
    pub parent: Option<SessionId>,
    /// Depth.
    pub depth: u32,
    /// Role name.
    pub role: String,
    /// Task text.
    pub task: String,
    /// Statements, in order.
    pub statement_ids: Vec<StatementId>,
    /// Children, in order of appearance.
    pub children: Vec<SessionId>,
    /// How it ended, if it did.
    pub outcome: Option<String>,
    /// Turns taken.
    pub turns: u32,
    /// Final usage (children included), once finished.
    pub calls: u64,
    /// Tokens.
    pub tokens: u64,
    /// Dollars.
    pub dollars: f64,
    /// Completed turns, in order.
    pub turns_done: Vec<TurnRecord>,
}

/// A run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunNode {
    /// Id.
    pub id: RunId,
    /// Task text.
    pub task: String,
    /// Root session, once started.
    pub root: Option<SessionId>,
    /// Outcome, once finished.
    pub outcome: Option<String>,
    /// The answer rendered, if any.
    pub answer: Option<String>,
}

/// Everything the TUI knows.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Model {
    /// Runs in order of appearance.
    pub run_order: Vec<RunId>,
    /// Runs.
    pub runs: HashMap<RunId, RunNode>,
    /// Sessions.
    pub sessions: HashMap<SessionId, SessionNode>,
    /// Statements.
    pub statements: HashMap<StatementId, StatementNode>,
    /// Calls.
    pub calls: HashMap<CallId, CallNode>,
    /// Statement of each call.
    pub call_owner: HashMap<CallId, StatementId>,
    /// Last cursor seen (for reconnecting).
    pub cursor: Option<Cursor>,
    /// Daemon generation.
    pub generation: Option<u64>,
    /// Totals over every finished call.
    pub total_calls: u64,
    /// Total memo hits.
    pub total_memo: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Total dollars.
    pub total_dollars: f64,
    /// Last table returned by an untagged `Query` (the trace explorer).
    pub table: Option<(Vec<String>, Vec<Vec<String>>)>,
    /// The task board (`tag = board`): generator, dial, pending, running, solved, failed, review.
    pub board: Option<(Vec<String>, Vec<Vec<String>>)>,
    /// Recent task outcomes (`tag = outcomes`), oldest first: 1 solved, 0 failed.
    pub outcomes: Vec<u64>,
    /// Recent tasks (`tag = tasks`): kind, generator, status, detail.
    pub tasks: Option<(Vec<String>, Vec<Vec<String>>)>,
    /// Last error or notice from the daemon.
    pub notice: Option<String>,
}

impl Model {
    /// Fold one server message in.
    pub fn apply(&mut self, msg: ServerMessage) {
        match msg {
            ServerMessage::Hello { generation, .. } => self.generation = Some(generation),
            ServerMessage::Event { cursor, traced } => {
                self.cursor = Some(cursor);
                self.apply_event(traced.event);
            }
            ServerMessage::CallDelta { call, text } => {
                if let Some(c) = self.calls.get_mut(&call) {
                    c.streaming.push_str(&text);
                }
            }
            ServerMessage::RunAccepted { run } => {
                self.runs.entry(run).or_insert_with(|| RunNode {
                    id: run,
                    task: String::new(),
                    root: None,
                    outcome: None,
                    answer: None,
                });
                if !self.run_order.contains(&run) {
                    self.run_order.push(run);
                }
            }
            ServerMessage::RunFinished {
                run,
                outcome,
                answer,
            } => {
                let node = self.runs.entry(run).or_insert_with(|| RunNode {
                    id: run,
                    task: String::new(),
                    root: None,
                    outcome: None,
                    answer: None,
                });
                node.outcome = Some(outcome);
                node.answer = answer;
                if !self.run_order.contains(&run) {
                    self.run_order.push(run);
                }
            }
            ServerMessage::Table { columns, rows, tag } => match tag.as_deref() {
                Some("board") => self.board = Some((columns, rows)),
                Some("outcomes") => {
                    self.outcomes = rows
                        .iter()
                        .filter_map(|r| r.first())
                        .map(|v| if v == "solved" { 1 } else { 0 })
                        .collect();
                }
                Some("tasks") => self.tasks = Some((columns, rows)),
                _ => self.table = Some((columns, rows)),
            },
            ServerMessage::Submitted { results, .. } => {
                self.notice = results.last().map(|r| r.text.clone());
            }
            ServerMessage::TurnFinished {
                session,
                turn,
                reply,
                sql,
                results,
            } => {
                if let Some(s) = self.sessions.get_mut(&session) {
                    s.turns_done.push(TurnRecord {
                        n: turn,
                        reply,
                        sql,
                        results,
                    });
                }
            }
            ServerMessage::Runs { runs } => {
                self.notice = Some(format!("{} live run(s)", runs.len()));
            }
            ServerMessage::Ok { message } => self.notice = Some(message),
            ServerMessage::Error { message } => self.notice = Some(format!("error: {message}")),
        }
    }

    fn apply_event(&mut self, event: TraceEvent) {
        match event {
            TraceEvent::RunStarted { run, task } => {
                let node = self.runs.entry(run).or_insert_with(|| RunNode {
                    id: run,
                    task: String::new(),
                    root: None,
                    outcome: None,
                    answer: None,
                });
                node.task = task;
                if !self.run_order.contains(&run) {
                    self.run_order.push(run);
                }
            }
            TraceEvent::SessionStarted {
                run,
                session,
                parent,
                depth,
                role,
                task,
            } => {
                self.sessions.insert(
                    session,
                    SessionNode {
                        id: session,
                        run,
                        parent,
                        depth,
                        role,
                        task,
                        statement_ids: vec![],
                        children: vec![],
                        outcome: None,
                        turns: 0,
                        calls: 0,
                        tokens: 0,
                        dollars: 0.0,
                        turns_done: vec![],
                    },
                );
                match parent {
                    Some(p) => {
                        if let Some(ps) = self.sessions.get_mut(&p) {
                            ps.children.push(session);
                        }
                    }
                    None => {
                        let node = self.runs.entry(run).or_insert_with(|| RunNode {
                            id: run,
                            task: String::new(),
                            root: None,
                            outcome: None,
                            answer: None,
                        });
                        node.root = Some(session);
                        if !self.run_order.contains(&run) {
                            self.run_order.push(run);
                        }
                    }
                }
            }
            TraceEvent::SessionFinished {
                session,
                outcome,
                turns,
                usage,
            } => {
                if let Some(s) = self.sessions.get_mut(&session) {
                    s.outcome = Some(outcome);
                    s.turns = turns;
                    s.calls = usage.calls;
                    s.tokens = usage.tokens;
                    s.dollars = usage.dollars;
                }
            }
            TraceEvent::StatementStarted {
                session,
                statement,
                sql,
            } => {
                self.statements.insert(
                    statement,
                    StatementNode {
                        id: statement,
                        sql,
                        finished: false,
                        rows: 0,
                        calls: 0,
                        tokens: 0,
                        dollars: 0.0,
                        elapsed: Duration::ZERO,
                        error: None,
                        explain: None,
                        call_ids: vec![],
                        tool_calls: vec![],
                        rounds: vec![],
                    },
                );
                if let Some(s) = self.sessions.get_mut(&session) {
                    s.statement_ids.push(statement);
                }
            }
            TraceEvent::StatementPlanned {
                statement, explain, ..
            } => {
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.explain = Some(explain);
                }
            }
            TraceEvent::StatementFinished {
                statement,
                rows,
                usage,
                error,
                elapsed,
            } => {
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.finished = true;
                    s.rows = rows;
                    s.calls = usage.calls;
                    s.tokens = usage.tokens;
                    s.dollars = usage.dollars;
                    s.elapsed = elapsed;
                    s.error = error;
                }
            }
            TraceEvent::CallStarted {
                statement,
                call,
                alias,
                ..
            } => {
                self.calls.insert(
                    call,
                    CallNode {
                        id: call,
                        alias,
                        finished: false,
                        memo_hit: false,
                        tokens: 0,
                        dollars: 0.0,
                        error: None,
                        streaming: String::new(),
                    },
                );
                self.call_owner.insert(call, statement);
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.call_ids.push(call);
                }
            }
            TraceEvent::CallFinished {
                call,
                input_tokens,
                output_tokens,
                cost_usd,
                memo_hit,
                error,
                ..
            } => {
                if let Some(c) = self.calls.get_mut(&call) {
                    c.finished = true;
                    c.memo_hit = memo_hit;
                    c.tokens = input_tokens + output_tokens;
                    c.dollars = cost_usd;
                    c.error = error;
                }
                if memo_hit {
                    self.total_memo += 1;
                } else {
                    self.total_calls += 1;
                }
                self.total_tokens += input_tokens + output_tokens;
                self.total_dollars += cost_usd;
            }
            TraceEvent::ToolCall {
                statement,
                tool,
                args,
                bytes_out,
                error,
                ..
            } => {
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.tool_calls.push(match error {
                        Some(e) => format!("{tool}({args}) -> error: {e}"),
                        None => format!("{tool}({args}) -> {bytes_out} bytes"),
                    });
                }
            }
            TraceEvent::RecursionRound {
                statement,
                cte,
                round,
                delta_rows,
                total_rows,
            } => {
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.rounds.push((cte, round, delta_rows, total_rows));
                }
            }
            TraceEvent::BudgetExceeded { statement, detail } => {
                if let Some(s) = self.statements.get_mut(&statement) {
                    s.error = Some(format!("budget exceeded: {detail}"));
                }
            }
            TraceEvent::Final { .. } => {}
        }
    }

    /// Root sessions in run order.
    pub fn roots(&self) -> Vec<SessionId> {
        self.run_order
            .iter()
            .filter_map(|r| self.runs.get(r).and_then(|n| n.root))
            .collect()
    }

    /// Whether any session is still running.
    pub fn busy(&self) -> bool {
        self.sessions.values().any(|s| s.outcome.is_none())
    }
}

/// One line of the call tree, flattened for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// A session.
    Session(SessionId),
    /// A statement.
    Statement(StatementId),
    /// A call.
    Call(CallId),
}

/// Which rows are folded.
#[derive(Debug, Default, Clone)]
pub struct Folds {
    sessions: std::collections::HashSet<SessionId>,
    statements: std::collections::HashSet<StatementId>,
}

impl Folds {
    /// Toggle a row's fold state.
    pub fn toggle(&mut self, row: &TreeRow) {
        match row {
            TreeRow::Session(s) => {
                if !self.sessions.remove(s) {
                    self.sessions.insert(*s);
                }
            }
            TreeRow::Statement(s) => {
                if !self.statements.remove(s) {
                    self.statements.insert(*s);
                }
            }
            TreeRow::Call(_) => {}
        }
    }

    /// Whether a row is folded.
    pub fn is_folded(&self, row: &TreeRow) -> bool {
        match row {
            TreeRow::Session(s) => self.sessions.contains(s),
            TreeRow::Statement(s) => self.statements.contains(s),
            TreeRow::Call(_) => false,
        }
    }
}

/// Flatten the tree into display rows (depth, row), honouring folds.
pub fn flatten(model: &Model, folds: &Folds) -> Vec<(usize, TreeRow)> {
    fn walk(
        model: &Model,
        folds: &Folds,
        session: SessionId,
        depth: usize,
        out: &mut Vec<(usize, TreeRow)>,
    ) {
        let Some(s) = model.sessions.get(&session) else {
            return;
        };
        out.push((depth, TreeRow::Session(session)));
        if folds.is_folded(&TreeRow::Session(session)) {
            return;
        }
        // Statements and child sessions interleave by statement order: a
        // child appears under the statement that spawned it when the
        // statement was running at the child's start; we approximate by
        // listing statements, then children spawned during them.
        let mut children = s.children.iter().copied().peekable();
        for stmt_id in &s.statement_ids {
            out.push((depth + 1, TreeRow::Statement(*stmt_id)));
            if folds.is_folded(&TreeRow::Statement(*stmt_id)) {
                continue;
            }
            if let Some(st) = model.statements.get(stmt_id) {
                for c in &st.call_ids {
                    out.push((depth + 2, TreeRow::Call(*c)));
                }
                // Children whose id sorts before the next statement's id were
                // spawned while this statement ran (ids are time-ordered).
                let next_stmt = s
                    .statement_ids
                    .iter()
                    .skip_while(|x| *x != stmt_id)
                    .nth(1)
                    .map(|x| x.0);
                while let Some(child) = children.peek().copied() {
                    let spawned_before_next = next_stmt.is_none_or(|n| child.0 < n);
                    if spawned_before_next && child.0 > stmt_id.0 {
                        children.next();
                        walk(model, folds, child, depth + 2, out);
                    } else {
                        break;
                    }
                }
            }
        }
        for child in children {
            walk(model, folds, child, depth + 1, out);
        }
    }
    let mut out = vec![];
    for root in model.roots() {
        walk(model, folds, root, 0, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::BudgetUsage;
    use kleene_trace::Traced;
    use std::time::SystemTime;

    fn ev(seq: u64, event: TraceEvent) -> ServerMessage {
        ServerMessage::Event {
            cursor: Cursor {
                generation: 1,
                sequence: seq,
            },
            traced: Traced {
                at: SystemTime::now(),
                event,
            },
        }
    }

    #[test]
    fn events_build_a_tree_with_totals() {
        let mut m = Model::default();
        let run = RunId::new();
        let root = SessionId::new();
        let stmt = StatementId::new();
        let call = CallId::new();
        let child = SessionId::new();
        m.apply(ev(
            0,
            TraceEvent::RunStarted {
                run,
                task: "t".into(),
            },
        ));
        m.apply(ev(
            1,
            TraceEvent::SessionStarted {
                run,
                session: root,
                parent: None,
                depth: 0,
                role: "root".into(),
                task: "t".into(),
            },
        ));
        m.apply(ev(
            2,
            TraceEvent::StatementStarted {
                session: root,
                statement: stmt,
                sql: "SELECT 1".into(),
            },
        ));
        m.apply(ev(
            3,
            TraceEvent::CallStarted {
                statement: stmt,
                call,
                alias: "worker".into(),
                model: String::new(),
                fingerprint: "f".into(),
            },
        ));
        m.apply(ev(
            4,
            TraceEvent::SessionStarted {
                run,
                session: child,
                parent: Some(root),
                depth: 1,
                role: "self".into(),
                task: "sub".into(),
            },
        ));
        m.apply(ev(
            5,
            TraceEvent::CallFinished {
                call,
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cost_usd: 0.01,
                elapsed: Duration::from_millis(3),
                memo_hit: false,
                error: None,
            },
        ));
        m.apply(ev(
            6,
            TraceEvent::StatementFinished {
                statement: stmt,
                rows: 1,
                usage: BudgetUsage {
                    calls: 1,
                    tokens: 15,
                    dollars: 0.01,
                    wall: Duration::ZERO,
                },
                error: None,
                elapsed: Duration::from_millis(5),
            },
        ));
        assert_eq!(m.roots(), vec![root]);
        assert_eq!(m.total_calls, 1);
        assert_eq!(m.total_tokens, 15);
        assert!(m.busy());
        let rows = flatten(&m, &Folds::default());
        assert_eq!(
            rows,
            vec![
                (0, TreeRow::Session(root)),
                (1, TreeRow::Statement(stmt)),
                (2, TreeRow::Call(call)),
                (2, TreeRow::Session(child)),
            ]
        );
        let mut folds = Folds::default();
        folds.toggle(&TreeRow::Statement(stmt));
        let rows = flatten(&m, &folds);
        assert_eq!(
            rows.len(),
            3,
            "folded statement hides its call, child moves under the session: {rows:?}"
        );
        assert_eq!(m.cursor.map(|c| c.sequence), Some(6));
    }
}
