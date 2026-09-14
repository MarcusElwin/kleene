//! Rendering: a pure function of the [`App`] state.
//!
//! Layout: a one-line header (brand, run, status, spend), the view, and a
//! footer of key chips. Panels are rounded and titled; the selected row is a
//! highlighted band; states are colour-coded through the [`Theme`].

use crate::model::{Model, TreeRow};
use crate::theme::{Theme, BRAND, TAGLINE};
use crate::{App, View};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, Wrap};
use ratatui::Frame;

/// Draw the whole screen.
pub fn draw(f: &mut Frame<'_>, app: &App) {
    let t = app.theme;
    let area = f.area();
    f.render_widget(Paragraph::new("").style(Style::default().bg(t.bg)), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, chunks[0], app);
    match app.view {
        View::Session => draw_session(f, chunks[1], app),
        View::Plan => draw_plan(f, chunks[1], app),
        View::Trace => draw_trace(f, chunks[1], app),
        View::Tasks => draw_tasks(f, chunks[1], app),
        View::Help => draw_help(f, chunks[1], app),
    }
    draw_footer(f, chunks[2], app);
}

fn short(id: impl ToString) -> String {
    let s = id.to_string();
    s[..s.len().min(8)].to_string()
}

fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn draw_header(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let m = &app.model;
    let run = m
        .run_order
        .last()
        .and_then(|r| m.runs.get(r))
        .map(|r| format!("{} · {}", short(r.id), first_line(&r.task, 48)))
        .unwrap_or_else(|| "no run yet".into());
    let (dot, status, color) = if !app.connected {
        ("✕", "disconnected", t.err)
    } else if m.busy() {
        ("●", "running", t.ok)
    } else {
        ("○", "idle", t.muted)
    };
    let memo_pct = if m.total_calls + m.total_memo > 0 {
        (m.total_memo as f64 / (m.total_calls + m.total_memo) as f64 * 100.0).round()
    } else {
        0.0
    };
    let left = Line::from(vec![
        Span::styled(
            format!(" {BRAND} "),
            Style::default()
                .fg(t.on_accent)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {run}  "), t.text()),
        Span::styled(
            format!("{dot} {status}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]);
    let right = Line::from(vec![
        Span::styled("λ ", Style::default().fg(t.accent)),
        Span::styled(format!("{} calls", m.total_calls), t.text()),
        Span::styled("  ⟳ ", Style::default().fg(t.accent)),
        Span::styled(format!("{memo_pct:.0}% memo"), t.text()),
        Span::styled("  ▤ ", Style::default().fg(t.accent)),
        Span::styled(format!("{} tok", human(m.total_tokens)), t.text()),
        Span::styled("  $ ", Style::default().fg(t.warn)),
        Span::styled(
            format!("{:.4}", m.total_dollars),
            Style::default().fg(t.warn).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {} ", t.name), t.dim()),
    ])
    .alignment(Alignment::Right);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(20),
            Constraint::Length(right.width() as u16),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(left).style(Style::default().bg(t.bg)),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(right).style(Style::default().bg(t.bg)),
        cols[1],
    );
}

fn draw_footer(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let chips: Vec<Span> = match app.view {
        View::Trace if app.editing => [
            t.chip("type", "SQL"),
            t.chip("⏎", "run"),
            t.chip("esc", "done"),
        ]
        .concat(),
        View::Trace => [
            t.chip("1-4", "views"),
            t.chip("e", "edit"),
            t.chip("r", "rerun"),
            t.chip("t", "theme"),
            t.chip("q", "detach"),
        ]
        .concat(),
        View::Tasks => [
            t.chip("1-4", "views"),
            t.chip("r", "refresh"),
            t.chip("t", "theme"),
            t.chip("q", "detach"),
        ]
        .concat(),
        _ => [
            t.chip("1-4", "views"),
            t.chip("j/k", "move"),
            t.chip("f", "fold"),
            t.chip("J/K", "scroll"),
            t.chip("x", "cancel"),
            t.chip("t", "theme"),
            t.chip("?", "help"),
            t.chip("q", "detach"),
        ]
        .concat(),
    };
    let notice = app
        .model
        .notice
        .as_deref()
        .map(|n| first_line(n, 60))
        .unwrap_or_default();
    let notice_line = Line::from(Span::styled(
        format!("{notice} "),
        Style::default().fg(t.warn),
    ))
    .alignment(Alignment::Right);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(notice_line.width() as u16),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(Line::from(chips)).style(Style::default().bg(t.bg)),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(notice_line).style(Style::default().bg(t.bg)),
        cols[1],
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
    if app.model.runs.is_empty() {
        draw_welcome(f, cols[1], app);
    } else {
        draw_transcript(f, cols[1], app);
    }
    if !app.compact {
        draw_sidebar(f, cols[2], app);
    }
}

/// Text and colour of one tree row.
pub fn tree_label(m: &Model, row: &TreeRow) -> (String, Color) {
    tree_label_themed(m, row, &Theme::default())
}

fn tree_label_themed(m: &Model, row: &TreeRow, t: &Theme) -> (String, Color) {
    match row {
        TreeRow::Session(id) => match m.sessions.get(id) {
            Some(s) => {
                let running = s.outcome.is_none();
                let dot = if running { "◐" } else { "■" };
                let tail = match &s.outcome {
                    Some(o) => format!("{o} · {} calls · ${:.3}", s.calls, s.dollars),
                    None => "running".into(),
                };
                let color = if running {
                    t.ok
                } else if s.outcome.as_deref() == Some("final") {
                    t.accent
                } else {
                    t.err
                };
                (
                    format!("{dot} {} {} d{} · {tail}", s.role, short(id), s.depth),
                    color,
                )
            }
            None => (format!("session {}", short(id)), t.muted),
        },
        TreeRow::Statement(id) => match m.statements.get(id) {
            Some(s) => {
                let dot = if s.finished { "▸" } else { "◐" };
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
                    t.state(!s.finished, s.error.is_some()),
                )
            }
            None => (format!("stmt {}", short(id)), t.muted),
        },
        TreeRow::Call(id) => match m.calls.get(id) {
            Some(c) => {
                let tail = if !c.finished {
                    "◐ running".to_string()
                } else if let Some(e) = &c.error {
                    format!("✗ {}", first_line(e, 30))
                } else if c.memo_hit {
                    "◇ memo".to_string()
                } else {
                    format!("{} tok · ${:.4}", c.tokens, c.dollars)
                };
                let color = if c.error.is_some() {
                    t.err
                } else if c.memo_hit {
                    t.muted
                } else {
                    t.warn
                };
                (format!("λ {} · {tail}", c.alias), color)
            }
            None => (format!("call {}", short(id)), t.muted),
        },
    }
}

fn draw_tree(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let rows = app.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, (depth, row))| {
            let (text, color) = tree_label_themed(&app.model, row, &t);
            let fold = if app.folds.is_folded(row) { "▸ " } else { "" };
            let guide = "│ ".repeat(*depth);
            let mut style = Style::default().fg(color);
            if i == app.selected {
                style = style.bg(t.sel_bg).add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::from(vec![
                Span::styled(guide, t.dim()),
                Span::styled(format!("{fold}{text}"), style),
            ]))
        })
        .collect();
    let live = app
        .model
        .sessions
        .values()
        .filter(|s| s.outcome.is_none())
        .count();
    let list = List::new(items).block(t.panel_with_meta(
        "Call tree",
        format!("{} rows · {live} live", rows.len()),
        app.view == View::Session,
    ));
    f.render_widget(list, area);
}

fn draw_welcome(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let block = t.panel("Welcome", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("╭──────────╮", Style::default().fg(t.accent))),
        Line::from(vec![
            Span::styled("│ ", Style::default().fg(t.accent)),
            Span::styled(BRAND, t.accent_text()),
            Span::styled(" │", Style::default().fg(t.accent)),
        ]),
        Line::from(Span::styled("╰──────────╯", Style::default().fg(t.accent))),
        Line::from(Span::styled(TAGLINE, t.dim())),
        Line::from(""),
        Line::from(Span::styled(
            "No run yet. Start one from a shell:",
            t.text(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  kleene tui --run \"Which project consumed the most hours?\" --context notes.txt",
            Style::default().fg(t.fg),
        )),
        Line::from(Span::styled(
            "  kleene run @task.txt --budget-calls 60",
            Style::default().fg(t.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Runs started anywhere against this daemon appear here as they happen.",
            t.dim(),
        )),
        Line::from(Span::styled(
            "3 opens the trace explorer over the store; 4 the task board; ? the keymap.",
            t.dim(),
        )),
    ];
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

/// The transcript text for the selected row.
pub fn transcript_text(app: &App) -> String {
    let m = &app.model;
    match app.selected_row() {
        None => {
            "No sessions yet. Start one with `kleene tui --run \"task\"` or `kleene run`.".into()
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
                out.push_str("◐ running\n");
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
                out.push_str("◐ running\n");
            }
            if !c.streaming.is_empty() {
                out.push('\n');
                out.push_str(&c.streaming);
            }
            out
        }
    }
}

/// Colour transcript lines by what they say: results, errors, fences, SQL.
fn styled_transcript(text: &str, t: &Theme) -> Text<'static> {
    let mut in_sql = false;
    let lines = text
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            if trimmed.starts_with("```") {
                in_sql = !in_sql;
                return Line::from(Span::styled(l.to_string(), t.dim()));
            }
            let style = if trimmed.starts_with('→') {
                Style::default().fg(t.ok)
            } else if trimmed.starts_with('✗') {
                Style::default().fg(t.err)
            } else if trimmed.starts_with('◐') {
                Style::default().fg(t.ok).add_modifier(Modifier::ITALIC)
            } else if trimmed == "ANSWER" || trimmed.starts_with("ended:") {
                t.accent_text()
            } else if trimmed.starts_with("session ")
                || trimmed.starts_with("task:")
                || trimmed.starts_with("call ")
                || trimmed.ends_with(':')
            {
                t.dim()
            } else if in_sql {
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD)
            } else {
                t.text()
            };
            Line::from(Span::styled(l.to_string(), style))
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

fn draw_transcript(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let text = transcript_text(app);
    let meta = match app.selected_row() {
        Some(TreeRow::Session(_)) => "session",
        Some(TreeRow::Statement(_)) => "statement",
        Some(TreeRow::Call(_)) => "model call",
        None => "",
    };
    let p = Paragraph::new(styled_transcript(&text, &t))
        .block(t.panel_with_meta("Transcript", meta, false))
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
    let t = app.theme;
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(9)])
        .split(area);
    let p = Paragraph::new(plan_text(app))
        .style(t.text())
        .block(t.panel("Plan", false))
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
    let block = t.panel("Metrics", false);
    let inner = block.inner(parts[1]);
    f.render_widget(block, parts[1]);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(t.accent).bg(t.sel_bg))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!("memo {:.0}%", ratio * 100.0));
    f.render_widget(gauge, rows[0]);
    let stats = vec![
        stat_line(
            &t,
            "sessions",
            format!("{} ({running} live)", m.sessions.len()),
        ),
        stat_line(&t, "depth", depth.to_string()),
        stat_line(
            &t,
            "calls",
            format!("{} + {} memo", m.total_calls, m.total_memo),
        ),
        stat_line(&t, "tokens", human(m.total_tokens)),
        stat_line(&t, "spend", format!("${:.4}", m.total_dollars)),
    ];
    f.render_widget(Paragraph::new(Text::from(stats)), rows[2]);
}

fn stat_line(t: &Theme, label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), t.dim()),
        Span::styled(value, t.text()),
    ])
}

fn draw_plan(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let p = Paragraph::new(plan_text(app))
        .style(t.text())
        .block(t.panel_with_meta("Plan", "selected statement", true))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_trace(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);
    let input_style = if app.editing {
        Style::default().fg(t.sel_fg).bg(t.sel_bg)
    } else {
        t.text()
    };
    let caret = if app.editing { "▏" } else { "" };
    let input = Paragraph::new(Line::from(vec![
        Span::styled("❯ ", Style::default().fg(t.accent)),
        Span::styled(format!("{}{caret}", app.query), input_style),
    ]))
    .block(t.panel_with_meta(
        "SQL over the store",
        if app.editing {
            "editing · ⏎ runs · esc done"
        } else {
            "e edit · r run"
        },
        app.editing,
    ));
    f.render_widget(input, parts[0]);
    match &app.model.table {
        Some((columns, rows)) => f.render_widget(
            table_widget(&t, columns, rows, "Result", format!("{} rows", rows.len()), 40),
            parts[1],
        ),
        None => f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(Span::styled("Press r to run the query.", t.text())),
                Line::from(""),
                Line::from(Span::styled(
                    "Tables: trace_runs, trace_sessions, trace_statements, trace_calls, trace_tool_calls, trace_rounds, memo, tasks, playbook, evals.",
                    t.dim(),
                )),
            ]))
            .wrap(Wrap { trim: false })
            .block(t.panel("Result", false)),
            parts[1],
        ),
    }
}

fn table_widget<'a>(
    t: &Theme,
    columns: &'a [String],
    rows: &'a [Vec<String>],
    title: &'a str,
    meta: String,
    max_cell: usize,
) -> Table<'a> {
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
                .min(max_cell) as u16;
            Constraint::Length(w + 1)
        })
        .collect();
    let header = Row::new(columns.iter().map(|c| Cell::from(c.as_str())))
        .style(t.accent_text().add_modifier(Modifier::UNDERLINED));
    let body: Vec<Row> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i % 2 == 1 {
                Style::default().fg(t.fg).bg(t.sel_bg)
            } else {
                t.text()
            };
            Row::new(r.iter().map(|v| Cell::from(first_line(v, max_cell)))).style(style)
        })
        .collect();
    Table::new(body, widths)
        .header(header)
        .block(t.panel_with_meta(title, meta, false))
}

fn draw_tasks(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let m = &app.model;
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Min(4),
        ])
        .split(area);
    let mut recent: Vec<u64> = m.outcomes.iter().rev().copied().collect();
    if recent.is_empty() {
        recent.push(0);
    }
    let solved: u64 = m.outcomes.iter().sum();
    let rate = if m.outcomes.is_empty() {
        0.0
    } else {
        solved as f64 / m.outcomes.len() as f64 * 100.0
    };
    let spark = Sparkline::default()
        .block(t.panel_with_meta(
            "Recent outcomes",
            format!("{solved} of {} solved · {rate:.0}%", m.outcomes.len()),
            false,
        ))
        .data(&recent)
        .max(1)
        .style(Style::default().fg(t.ok));
    f.render_widget(spark, parts[0]);
    match &m.board {
        Some((cols, rows)) => f.render_widget(
            table_widget(
                &t,
                cols,
                rows,
                "Board",
                "pending / running / solved / failed / review · dial per generator".into(),
                20,
            ),
            parts[1],
        ),
        None => f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(Span::styled("No tasks yet.", t.text())),
                Line::from(Span::styled(
                    "`kleene learn generate puzzle` or `kleene learn run` fills the board; r refreshes.",
                    t.dim(),
                )),
            ]))
            .wrap(Wrap { trim: false })
            .block(t.panel("Board", false)),
            parts[1],
        ),
    }
    match &m.tasks {
        Some((cols, rows)) => f.render_widget(
            table_widget(
                &t,
                cols,
                rows,
                "Recent tasks",
                format!("{}", rows.len()),
                48,
            ),
            parts[2],
        ),
        None => f.render_widget(
            Paragraph::new("").block(t.panel("Recent tasks", false)),
            parts[2],
        ),
    }
}

fn draw_help(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let rows: [(&str, &str); 10] = [
        ("Views", "1 session · 2 plan · 3 trace · 4 tasks · ? help"),
        ("Move", "j/k or arrows select a row in the call tree"),
        (
            "Fold",
            "f, Enter or Space folds the selected session or statement",
        ),
        ("Scroll", "J/K or PageUp/PageDown scroll the transcript"),
        (
            "Cancel",
            "x or Esc cancels the selected statement; a root session row cancels its run",
        ),
        ("Trace", "e or / edits the SQL, Enter runs it, r reruns it"),
        (
            "Tasks",
            "the continual loop's board (kleene learn); r refreshes, auto every 2s",
        ),
        ("Theme", "t switches between the dark and light palettes"),
        (
            "Leave",
            "d, q or Ctrl-C detaches; the daemon and its runs keep going",
        ),
        (
            "Setup",
            "kleene setup (in a shell) adds or changes provider keys",
        ),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<8}"), t.accent_text()),
                Span::styled((*v).to_string(), t.text()),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "The tree shows sessions (■/◐ role id depth), their statements (▸/◐) and model calls (λ, ◇ when served from the memo). Spend, tokens and the memo hit rate accumulate in the header.",
        t.dim(),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(t.panel("Keymap", true)),
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
    use kleene_core::{RunId, SessionId, StatementId};
    use kleene_daemon::{Cursor, ServerMessage};
    use kleene_trace::{TraceEvent, Traced};
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
        assert!(text.contains("Call tree"), "{text}");
        assert!(text.contains("root"), "{text}");
        assert!(text.contains("Find the bug"), "{text}");
        assert!(text.contains("Plan"), "{text}");
        assert!(text.contains("running"), "{text}");
        assert!(text.contains("kleene"), "brand in the header: {text}");
    }

    #[test]
    fn empty_state_shows_the_welcome_card() {
        let app = App::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("Welcome"), "{text}");
        assert!(text.contains("kleene tui --run"), "{text}");
    }

    #[test]
    fn keys_move_fold_cancel_and_theme() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with_run();
        assert_eq!(app.rows().len(), 2);
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.key(down), crate::Action::None);
        assert!(matches!(app.selected_row(), Some(TreeRow::Statement(_))));
        let cancel = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(
            app.key(cancel),
            crate::Action::Send(kleene_daemon::ClientRequest::Cancel { .. })
        ));
        let up = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        app.key(up);
        let fold = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        app.key(fold);
        assert_eq!(app.rows().len(), 1, "folded session hides its statement");
        let theme = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        assert_eq!(app.key(theme), crate::Action::None);
        assert_eq!(app.theme.name, "light");
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.key(quit), crate::Action::Detach);
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
            crate::Action::Send(kleene_daemon::ClientRequest::Query { sql, .. }) if sql == "S"
        ));
    }

    #[test]
    fn every_view_renders_in_both_layouts_and_themes() {
        let mut app = app_with_run();
        for (compact, size) in [(true, (80, 24)), (false, (140, 40))] {
            app.compact = compact;
            let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
            for theme in [Theme::dark(), Theme::light()] {
                app.theme = theme;
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
