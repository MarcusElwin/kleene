//! Rendering: a pure function of the [`App`] state.

use crate::model::{Model, TreeRow};
use crate::{App, View};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, List, ListItem, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

/// Draw the whole screen.
pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);
    draw_title(f, chunks[0], app);
    match app.view {
        View::Session => draw_session(f, chunks[1], app),
        View::Plan => draw_plan(f, chunks[1], app),
        View::Trace => draw_trace(f, chunks[1], app),
        View::Tasks => draw_placeholder(
            f,
            chunks[1],
            "Tasks",
            "The continual harness task board arrives in M6.",
        ),
        View::Help => draw_help(f, chunks[1]),
    }
    draw_keybar(f, chunks[2], app);
}

fn short(id: impl ToString) -> String {
    let s = id.to_string();
    s[..s.len().min(8)].to_string()
}

fn draw_title(f: &mut Frame<'_>, area: Rect, app: &App) {
    let m = &app.model;
    let run = m
        .run_order
        .last()
        .and_then(|r| m.runs.get(r))
        .map(|r| format!("run {} · {}", short(r.id), first_line(&r.task, 40)))
        .unwrap_or_else(|| "no runs yet".into());
    let status = if !app.connected {
        "disconnected"
    } else if m.busy() {
        "running"
    } else {
        "idle"
    };
    let memo_pct = if m.total_calls + m.total_memo > 0 {
        (m.total_memo as f64 / (m.total_calls + m.total_memo) as f64 * 100.0).round()
    } else {
        0.0
    };
    let line = Line::from(vec![
        Span::styled(
            " callgebra ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {run} · {status} · ")),
        Span::styled(
            format!(
                "{} calls · {} memo ({memo_pct:.0}%) · {} tok · ${:.4}",
                m.total_calls, m.total_memo, m.total_tokens, m.total_dollars
            ),
            Style::default().fg(Color::Yellow),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_keybar(f: &mut Frame<'_>, area: Rect, app: &App) {
    let keys = match app.view {
        View::Trace if app.editing => "type SQL  ⏎ run  Esc done",
        View::Trace => "1 session  2 plan  3 trace  4 tasks  ? help │ e edit  r rerun  q detach",
        _ => "1 session  2 plan  3 trace  4 tasks  ? help │ j/k move  f fold  J/K scroll  x cancel  d detach  q quit",
    };
    let notice = app
        .model
        .notice
        .as_deref()
        .map(|n| format!(" │ {}", first_line(n, 60)))
        .unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(keys, Style::default().fg(Color::DarkGray)),
            Span::styled(notice, Style::default().fg(Color::Magenta)),
        ])),
        area,
    );
}

fn draw_session(f: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = if app.compact {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(45),
                Constraint::Percentage(25),
            ])
            .split(area)
    };
    draw_tree(f, cols[0], app);
    draw_transcript(f, cols[1], app);
    if !app.compact {
        draw_sidebar(f, cols[2], app);
    }
}

/// Text of one tree row.
pub fn tree_label(m: &Model, row: &TreeRow) -> (String, Color) {
    match row {
        TreeRow::Session(id) => match m.sessions.get(id) {
            Some(s) => {
                let dot = if s.outcome.is_none() { "●" } else { "■" };
                let tail = match &s.outcome {
                    Some(o) => format!("{o} · {} calls · ${:.3}", s.calls, s.dollars),
                    None => "running".into(),
                };
                (
                    format!("{dot} {} {} d{} · {tail}", s.role, short(id), s.depth),
                    if s.outcome.is_none() {
                        Color::Green
                    } else if s.outcome.as_deref() == Some("final") {
                        Color::Cyan
                    } else {
                        Color::Red
                    },
                )
            }
            None => (format!("session {}", short(id)), Color::Gray),
        },
        TreeRow::Statement(id) => match m.statements.get(id) {
            Some(s) => {
                let dot = if s.finished { "▸" } else { "●" };
                let tail = if s.finished {
                    match &s.error {
                        Some(e) => format!("✗ {}", first_line(e, 40)),
                        None => format!(
                            "{} rows · {} calls · {:.1}s",
                            s.rows,
                            s.calls,
                            s.elapsed.as_secs_f64()
                        ),
                    }
                } else {
                    "running".into()
                };
                (
                    format!("{dot} {} · {tail}", first_line(&s.sql, 40)),
                    if s.error.is_some() {
                        Color::Red
                    } else if s.finished {
                        Color::White
                    } else {
                        Color::Green
                    },
                )
            }
            None => (format!("stmt {}", short(id)), Color::Gray),
        },
        TreeRow::Call(id) => match m.calls.get(id) {
            Some(c) => {
                let tail = if !c.finished {
                    "● running".to_string()
                } else if let Some(e) = &c.error {
                    format!("✗ {}", first_line(e, 30))
                } else if c.memo_hit {
                    "memo".to_string()
                } else {
                    format!("{} tok · ${:.4}", c.tokens, c.dollars)
                };
                (format!("λ {} · {tail}", c.alias), Color::Magenta)
            }
            None => (format!("call {}", short(id)), Color::Gray),
        },
    }
}

fn draw_tree(f: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = app.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, (depth, row))| {
            let (text, color) = tree_label(&app.model, row);
            let fold = if app.folds.is_folded(row) { "▸ " } else { "" };
            let mut style = Style::default().fg(color);
            if i == app.selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(Line::from(Span::styled(
                format!("{}{fold}{text}", "  ".repeat(*depth)),
                style,
            )))
        })
        .collect();
    let title = format!(" CALL TREE ({} rows) ", rows.len());
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(list, area);
}

/// The transcript text for the selected row.
pub fn transcript_text(app: &App) -> String {
    let m = &app.model;
    match app.selected_row() {
        None => {
            "No sessions yet. Start one with `callgebra tui --run \"task\"` or `callgebra run`."
                .into()
        }
        Some(TreeRow::Session(id)) => {
            let Some(s) = m.sessions.get(&id) else {
                return String::new();
            };
            let mut out = format!(
                "session {}  role {}  depth {}\ntask: {}\n\n",
                s.id, s.role, s.depth, s.task
            );
            for sid in &s.statement_ids {
                if let Some(st) = m.statements.get(sid) {
                    out.push_str(&format!("{}\n", st.sql.trim()));
                    if let Some(e) = &st.error {
                        out.push_str(&format!("  ✗ {e}\n"));
                    } else if st.finished {
                        out.push_str(&format!(
                            "  → {} rows · {} calls · {} tok · ${:.4} · {:.1}s\n",
                            st.rows,
                            st.calls,
                            st.tokens,
                            st.dollars,
                            st.elapsed.as_secs_f64()
                        ));
                    }
                    out.push('\n');
                }
            }
            if let Some(o) = &s.outcome {
                out.push_str(&format!(
                    "ended: {o} after {} turns · {} calls · {} tok · ${:.4}\n",
                    s.turns, s.calls, s.tokens, s.dollars
                ));
            }
            if let Some(run) = m.runs.get(&s.run) {
                if s.parent.is_none() {
                    if let Some(a) = &run.answer {
                        out.push_str(&format!("\nANSWER\n{a}"));
                    }
                }
            }
            out
        }
        Some(TreeRow::Statement(id)) => {
            let Some(st) = m.statements.get(&id) else {
                return String::new();
            };
            let mut out = format!("```sql\n{}\n```\n", st.sql.trim());
            if let Some(e) = &st.error {
                out.push_str(&format!("✗ {e}\n"));
            } else if st.finished {
                out.push_str(&format!(
                    "→ {} rows · {} calls · {} tok · ${:.4} · {:.1}s\n",
                    st.rows,
                    st.calls,
                    st.tokens,
                    st.dollars,
                    st.elapsed.as_secs_f64()
                ));
            } else {
                out.push_str("● running\n");
            }
            if !st.tool_calls.is_empty() {
                out.push_str("\ntools:\n");
                for t in &st.tool_calls {
                    out.push_str(&format!("  {t}\n"));
                }
            }
            if !st.rounds.is_empty() {
                out.push_str("\nrecursion:\n");
                for (cte, round, delta, total) in &st.rounds {
                    out.push_str(&format!(
                        "  μ {cte} round {round}: +{delta} rows, {total} total\n"
                    ));
                }
            }
            if !st.call_ids.is_empty() {
                out.push_str(&format!("\n{} model call(s)\n", st.call_ids.len()));
            }
            out
        }
        Some(TreeRow::Call(id)) => {
            let Some(c) = m.calls.get(&id) else {
                return String::new();
            };
            let mut out = format!("call {}  alias {}\n", c.id, c.alias);
            if c.finished {
                out.push_str(&match &c.error {
                    Some(e) => format!("✗ {e}\n"),
                    None if c.memo_hit => "served from the memo\n".to_string(),
                    None => format!("{} tokens · ${:.4}\n", c.tokens, c.dollars),
                });
            } else {
                out.push_str("● running\n");
            }
            if !c.streaming.is_empty() {
                out.push('\n');
                out.push_str(&c.streaming);
            }
            out
        }
    }
}

fn draw_transcript(f: &mut Frame<'_>, area: Rect, app: &App) {
    let text = transcript_text(app);
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" TRANSCRIPT "))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(p, area);
}

/// The plan text for the selected row (its statement's `EXPLAIN`).
pub fn plan_text(app: &App) -> String {
    let m = &app.model;
    let stmt = match app.selected_row() {
        Some(TreeRow::Statement(s)) => Some(s),
        Some(TreeRow::Call(c)) => m.call_owner.get(&c).copied(),
        Some(TreeRow::Session(s)) => m
            .sessions
            .get(&s)
            .and_then(|n| n.statement_ids.last().copied()),
        None => None,
    };
    match stmt.and_then(|s| m.statements.get(&s)) {
        Some(st) => match &st.explain {
            Some(e) => {
                let mut out = e.clone();
                if st.finished {
                    out.push_str(&format!(
                        "\nactual: {} rows, {} calls, {} tok, ${:.4}",
                        st.rows, st.calls, st.tokens, st.dollars
                    ));
                }
                out
            }
            None => "no plan recorded for this statement (EXPLAIN it to see one)".into(),
        },
        None => "select a statement".into(),
    }
}

fn draw_sidebar(f: &mut Frame<'_>, area: Rect, app: &App) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(6)])
        .split(area);
    let p = Paragraph::new(plan_text(app))
        .block(Block::default().borders(Borders::ALL).title(" PLAN "))
        .wrap(Wrap { trim: false });
    f.render_widget(p, parts[0]);
    let m = &app.model;
    let total = m.total_calls + m.total_memo;
    let ratio = if total > 0 {
        m.total_memo as f64 / total as f64
    } else {
        0.0
    };
    let running = m.sessions.values().filter(|s| s.outcome.is_none()).count();
    let depth = m.sessions.values().map(|s| s.depth).max().unwrap_or(0);
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" memo hit rate · {running} live · depth {depth} ")),
        )
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!("{:.0}%", ratio * 100.0));
    f.render_widget(gauge, parts[1]);
}

fn draw_plan(f: &mut Frame<'_>, area: Rect, app: &App) {
    let p = Paragraph::new(plan_text(app))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PLAN (selected statement) "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_trace(f: &mut Frame<'_>, area: Rect, app: &App) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);
    let style = if app.editing {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let input = Paragraph::new(app.query.as_str()).style(style).block(
        Block::default()
            .borders(Borders::ALL)
            .title(if app.editing {
                " SQL (editing) "
            } else {
                " SQL (e to edit, r to run) "
            }),
    );
    f.render_widget(input, parts[0]);
    match &app.model.table {
        Some((columns, rows)) => {
            let widths: Vec<Constraint> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let w = rows
                        .iter()
                        .map(|r| r.get(i).map(|v| v.chars().count()).unwrap_or(0))
                        .max()
                        .unwrap_or(0)
                        .max(c.chars().count())
                        .min(40) as u16;
                    Constraint::Length(w + 1)
                })
                .collect();
            let header = Row::new(columns.iter().map(|c| Cell::from(c.as_str())))
                .style(Style::default().add_modifier(Modifier::BOLD));
            let body: Vec<Row> = rows
                .iter()
                .map(|r| Row::new(r.iter().map(|v| Cell::from(first_line(v, 40)))))
                .collect();
            let table = Table::new(body, widths).header(header).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} rows ", rows.len())),
            );
            f.render_widget(table, parts[1]);
        }
        None => f.render_widget(
            Paragraph::new("Press r to run the query. Saved queries: trace_sessions, trace_statements, trace_calls, memo.")
                .block(Block::default().borders(Borders::ALL).title(" RESULT ")),
            parts[1],
        ),
    }
}

fn draw_help(f: &mut Frame<'_>, area: Rect) {
    let text = "\
Views      1 session   2 plan   3 trace   4 tasks   ? help
Move       j/k or arrows select a row in the call tree
Fold       f, Enter or Space folds the selected session or statement
Scroll     J/K scroll the transcript
Cancel     x or Esc cancels the selected statement (a session row cancels its run)
Trace      e edit the SQL, Enter runs it, r reruns it
Leave      d or q detaches; the daemon and its runs keep going

The tree shows sessions (■/● role id depth), their statements (▸/●) and
model calls (λ). Dollars, tokens and memo hits accumulate in the title bar.";
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" HELP ")),
        area,
    );
}

fn draw_placeholder(f: &mut Frame<'_>, area: Rect, title: &str, body: &str) {
    f.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        ),
        area,
    );
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().next().unwrap_or_default().trim();
    if l.chars().count() > max {
        format!(
            "{}…",
            l.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    } else {
        l.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callgebra_core::{RunId, SessionId, StatementId};
    use callgebra_daemon::{Cursor, ServerMessage};
    use callgebra_trace::{TraceEvent, Traced};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn ev(seq: u64, event: TraceEvent) -> ServerMessage {
        ServerMessage::Event {
            cursor: Cursor {
                generation: 1,
                sequence: seq,
            },
            traced: Traced {
                at: std::time::SystemTime::now(),
                event,
            },
        }
    }

    fn app_with_run() -> App {
        let mut app = App::default();
        let run = RunId::new();
        let root = SessionId::new();
        let stmt = StatementId::new();
        app.apply(ev(
            0,
            TraceEvent::RunStarted {
                run,
                task: "Find the bug".into(),
            },
        ));
        app.apply(ev(
            1,
            TraceEvent::SessionStarted {
                run,
                session: root,
                parent: None,
                depth: 0,
                role: "root".into(),
                task: "Find the bug".into(),
            },
        ));
        app.apply(ev(
            2,
            TraceEvent::StatementStarted {
                session: root,
                statement: stmt,
                sql: "SELECT COUNT(*) FROM ctx".into(),
            },
        ));
        app.apply(ev(
            3,
            TraceEvent::StatementPlanned {
                statement: stmt,
                explain: "π count\n  scan ctx".into(),
                estimate: serde_json::json!({}),
            },
        ));
        app
    }

    #[test]
    fn session_view_renders_tree_transcript_and_plan() {
        let app = app_with_run();
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("CALL TREE"), "{text}");
        assert!(text.contains("root"), "{text}");
        assert!(text.contains("Find the bug"), "{text}");
        assert!(text.contains("PLAN"), "{text}");
        assert!(text.contains("running"), "{text}");
    }

    #[test]
    fn keys_move_fold_and_cancel() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with_run();
        assert_eq!(app.rows().len(), 2);
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.key(down), crate::Action::None);
        assert!(matches!(app.selected_row(), Some(TreeRow::Statement(_))));
        let cancel = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(
            app.key(cancel),
            crate::Action::Send(callgebra_daemon::ClientRequest::Cancel { .. })
        ));
        let up = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        app.key(up);
        let fold = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        app.key(fold);
        assert_eq!(app.rows().len(), 1, "folded session hides its statement");
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.key(quit), crate::Action::Detach);
        // Plan view shows the explain text; the trace view takes SQL input.
        app.view = View::Plan;
        assert!(plan_text(&app).contains("scan ctx"));
        app.view = View::Trace;
        app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert!(app.editing);
        app.query.clear();
        app.key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            app.key(enter),
            crate::Action::Send(callgebra_daemon::ClientRequest::Query { sql }) if sql == "S"
        ));
    }

    #[test]
    fn compact_layout_renders_without_panicking() {
        let mut app = app_with_run();
        app.compact = true;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        for view in [
            View::Session,
            View::Plan,
            View::Trace,
            View::Tasks,
            View::Help,
        ] {
            app.view = view;
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
    }

    fn buffer_text(backend: &TestBackend) -> String {
        let buf = backend.buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
