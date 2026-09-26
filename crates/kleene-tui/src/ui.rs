//! Rendering: a pure function of the [`App`] state.
//!
//! One scrolling stream between a header and a prompt bar. The stream holds
//! the welcome block, what the user typed, every run as it happens (turns,
//! streamed replies, results, child sessions, the answer) and the output of
//! slash commands. Everything is built as lines from the model, so it renders
//! into a test buffer.

use crate::model::{Model, SessionNode};
use crate::theme::{Theme, BRAND, TAGLINE, WORDMARK_WIDTH};
use crate::{App, Entry, COMMANDS};
use kleene_core::RunId;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};
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
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, chunks[0], app);
    draw_stream(f, chunks[1], app);
    draw_prompt(f, chunks[2], app);
    draw_footer(f, chunks[3], app);
    draw_completions(f, chunks[1], chunks[2], app);
    if let Some(wizard) = &app.setup {
        crate::setup::draw(f, wizard);
    }
}

/// The tail of an id: UUID v7 starts with a timestamp, so the head is the
/// same for every id minted in the same run.
pub fn short(id: impl ToString) -> String {
    let s = id.to_string();
    s[s.len().saturating_sub(6)..].to_string()
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

fn draw_header(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let m = &app.model;
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
    let right = Line::from(vec![
        Span::styled("λ ", Style::default().fg(t.call)),
        Span::styled(format!("{} calls", m.total_calls), t.text()),
        Span::styled("  ⟳ ", Style::default().fg(t.accent)),
        Span::styled(format!("{memo_pct:.0}% memo"), t.text()),
        Span::styled("  ▤ ", Style::default().fg(t.accent)),
        Span::styled(format!("{} tok", human(m.total_tokens)), t.text()),
        Span::styled("  $ ", Style::default().fg(t.money)),
        Span::styled(
            format!("{:.4}", m.total_dollars),
            Style::default().fg(t.money).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {} ", t.name), t.dim()),
    ])
    .alignment(Alignment::Right);
    // Room for the task: the width minus the brand, the status and the right
    // side, so the status never gets clipped.
    let right_width = right.width();
    // Brand chip (10), gaps (6), id and separator (9), status (up to 14), a
    // gap before the totals (2), the ellipsis (1).
    let room = (area.width as usize)
        .saturating_sub(right_width + BRAND.chars().count() + status.len() + 30)
        .max(12);
    let run = app
        .followed()
        .and_then(|r| m.runs.get(&r))
        .map(|r| format!("{} · {}", short(r.id), first_line(&r.task, room)))
        .unwrap_or_else(|| "no run yet".into());
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
            format!("{dot} {status}  "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(20),
            Constraint::Length(right.width() as u16),
        ])
        .split(area);
    f.render_widget(
        Paragraph::new(left).style(Style::default().bg(t.band)),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(right).style(Style::default().bg(t.band)),
        cols[1],
    );
}

fn draw_footer(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let chips: Vec<Span> = [
        t.chip("⏎", "run"),
        t.chip("/", "commands"),
        t.chip("↑↓", "history"),
        t.chip("pgup/pgdn", "scroll"),
        t.chip("^p", "plans"),
        t.chip("^t", "theme"),
        t.chip("^x", "cancel"),
        t.chip("^c", "detach"),
    ]
    .concat();
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
        Paragraph::new(Line::from(chips)).style(Style::default().bg(t.band)),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(notice_line).style(Style::default().bg(t.band)),
        cols[1],
    );
}

/// The prompt bar: always focused; a task starts a run, `/…` is a command.
fn draw_prompt(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let (text, style) = if app.input.is_empty() {
        (
            "Describe a task and press Enter · / for commands".to_string(),
            Style::default().fg(t.faint),
        )
    } else if app.input.starts_with('/') {
        (app.input.clone(), Style::default().fg(t.accent))
    } else {
        (app.input.clone(), t.text())
    };
    let line = Line::from(vec![
        Span::styled(
            "❯ ",
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{text}▏"), style),
    ]);
    let meta = if app.scroll > 0 {
        format!("scrolled ↑{} · end returns", app.scroll)
    } else if app.followed().is_some() {
        "following".to_string()
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(line).block(t.panel_with_meta("Task", meta, true)),
        area,
    );
}

/// The command popup above the prompt while the input starts with `/`.
fn draw_completions(f: &mut Frame<'_>, stream: Rect, prompt: Rect, app: &App) {
    let matches = app.completions();
    if matches.is_empty() {
        return;
    }
    let t = app.theme;
    let width = 56u16.min(stream.width.saturating_sub(4)).max(20);
    let height = (matches.len() as u16 + 2).min(stream.height);
    let area = Rect {
        x: prompt.x + 2,
        y: prompt.y.saturating_sub(height),
        width,
        height,
    };
    f.render_widget(Clear, area);
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let selected = i == app.completion % matches.len().max(1);
            let style = if selected {
                Style::default()
                    .fg(t.sel_fg)
                    .bg(t.sel_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                t.text()
            };
            Line::from(vec![
                Span::styled(format!("{:<9}", c.name), style),
                Span::styled(format!("{:<12}", c.args), Style::default().fg(t.faint)),
                Span::styled(c.help, t.dim()),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(Text::from(lines)).block(t.panel("Commands · tab completes", false)),
        area,
    );
}

fn draw_stream(f: &mut Frame<'_>, area: Rect, app: &App) {
    let t = app.theme;
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    let lines = stream_lines(app, inner.width as usize);
    // Wrapped height is what scrolling must count, so wrap here by width.
    let wrapped = wrap_lines(lines, inner.width as usize);
    let total = wrapped.len();
    let visible = inner.height as usize;
    let bottom = total.saturating_sub(app.scroll.min(total.saturating_sub(visible)));
    let top = bottom.saturating_sub(visible);
    let shown: Vec<Line> = wrapped[top..bottom].to_vec();
    f.render_widget(
        Paragraph::new(Text::from(shown)).style(Style::default().bg(t.bg)),
        inner,
    );
}

/// Soft-wrap styled lines at `width` columns, keeping each span's style.
fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= width {
            out.push(line);
            continue;
        }
        let mut current: Vec<Span<'static>> = Vec::new();
        let mut used = 0usize;
        for span in line.spans {
            let style = span.style;
            let mut chunk = String::new();
            for ch in span.content.chars() {
                if used == width {
                    current.push(Span::styled(std::mem::take(&mut chunk), style));
                    out.push(Line::from(std::mem::take(&mut current)));
                    used = 0;
                }
                chunk.push(ch);
                used += 1;
            }
            if !chunk.is_empty() {
                current.push(Span::styled(chunk, style));
            }
        }
        if !current.is_empty() {
            out.push(Line::from(current));
        }
    }
    out
}

/// Every line of the stream, in order.
pub fn stream_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let mut out: Vec<Line<'static>> = Vec::new();
    for entry in &app.entries {
        match entry {
            Entry::Welcome => welcome(&t, width, &mut out),
            Entry::Input(text) => {
                out.push(Line::from(""));
                out.push(Line::from(vec![
                    Span::styled(
                        "❯ ",
                        Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(text.clone(), t.text().add_modifier(Modifier::BOLD)),
                ]));
            }
            Entry::Run(run) => run_lines(app, *run, &mut out),
            Entry::Table {
                title,
                columns,
                rows,
            } => {
                out.push(Line::from(""));
                out.push(Line::from(vec![
                    Span::styled(format!(" {title} "), chip(&t)),
                    Span::styled(
                        format!(
                            "  {} row{}",
                            rows.len(),
                            if rows.len() == 1 { "" } else { "s" }
                        ),
                        t.dim(),
                    ),
                ]));
                out.extend(table_lines(
                    &t,
                    columns,
                    rows,
                    40,
                    width.saturating_sub(2),
                    100,
                ));
            }
            Entry::Notice(text) => {
                out.push(Line::from(""));
                out.push(Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(t.warn)),
                    Span::styled(text.clone(), Style::default().fg(t.warn)),
                ]));
            }
            Entry::Help => help_lines(&t, &mut out),
            Entry::Text(text) => {
                out.push(Line::from(""));
                for l in text.lines() {
                    out.push(Line::from(Span::styled(l.to_string(), t.text())));
                }
            }
            Entry::Results(results) => {
                for r in results {
                    out.extend(result_lines(&t, &r.text, r.is_error, "  "));
                }
            }
        }
    }
    out
}

fn chip(t: &Theme) -> Style {
    Style::default()
        .fg(t.on_accent)
        .bg(t.accent)
        .add_modifier(Modifier::BOLD)
}

fn rule(t: &Theme, prefix: &str, label: String, right: String, width: usize) -> Line<'static> {
    let left = format!("── {label} ");
    let fill = width
        .saturating_sub(prefix.chars().count() + left.chars().count() + right.chars().count() + 2)
        .max(4);
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(t.border)),
        Span::styled(left, Style::default().fg(t.faint)),
        Span::styled("─".repeat(fill), Style::default().fg(t.border)),
        Span::styled(format!(" {right} "), t.dim()),
    ])
}

fn welcome(t: &Theme, width: usize, out: &mut Vec<Line<'static>>) {
    out.push(Line::from(""));
    if width >= WORDMARK_WIDTH as usize + 2 {
        out.extend(t.wordmark());
    } else {
        out.push(Line::from(Span::styled(BRAND, t.accent_text())));
        out.push(Line::from(Span::styled(TAGLINE, t.dim())));
    }
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Describe a task below and press Enter. The model writes CallSQL turn by turn; you watch it think,",
        t.text(),
    )));
    out.push(Line::from(Span::styled(
        "see every statement's plan and cost, and get the answer as a relation.",
        t.text(),
    )));
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled("  /sql ", Style::default().fg(t.accent)),
        Span::styled("SELECT COUNT(*) FROM ctx", Style::default().fg(t.sql)),
        Span::styled("        run one statement yourself", t.dim()),
    ]));
    out.push(Line::from(vec![
        Span::styled("  /trace ", Style::default().fg(t.accent)),
        Span::styled("SELECT * FROM trace_sessions", Style::default().fg(t.sql)),
        Span::styled("   query the trace, memo and task tables", t.dim()),
    ]));
    out.push(Line::from(vec![
        Span::styled("  /help", Style::default().fg(t.accent)),
        Span::styled(
            "                                 every command and key",
            t.dim(),
        ),
    ]));
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Runs started from a shell against this daemon appear here too. /setup adds or changes API keys.",
        t.dim(),
    )));
}

fn help_lines(t: &Theme, out: &mut Vec<Line<'static>>) {
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(" Commands ", chip(t))));
    for c in COMMANDS {
        out.push(Line::from(vec![
            Span::styled(format!("  {:<9}", c.name), Style::default().fg(t.accent)),
            Span::styled(format!("{:<14}", c.args), Style::default().fg(t.faint)),
            Span::styled(c.help, t.text()),
        ]));
    }
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(" Keys ", chip(t))));
    for (k, v) in [
        ("Enter", "run the task, or the command"),
        ("Tab", "complete the command"),
        ("↑ / ↓", "walk the input history"),
        (
            "PgUp / PgDn, Home / End",
            "scroll the stream; End follows again",
        ),
        ("Ctrl-P", "show or hide EXPLAIN plans under statements"),
        ("Ctrl-T", "next Catppuccin flavour"),
        ("Ctrl-X", "cancel the run being followed"),
        ("Ctrl-U", "clear the input"),
        ("Ctrl-C", "detach; the daemon and its runs keep going"),
    ] {
        out.push(Line::from(vec![
            Span::styled(format!("  {k:<24}"), Style::default().fg(t.accent)),
            Span::styled(v, t.text()),
        ]));
    }
}

/// A run: its task, the root session's turns, children, and the outcome.
fn run_lines(app: &App, run: RunId, out: &mut Vec<Line<'static>>) {
    let t = app.theme;
    let m = &app.model;
    let Some(r) = m.runs.get(&run) else {
        return;
    };
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled(format!(" run {} ", short(run)), chip(&t)),
        Span::styled(format!("  {}", first_line(&r.task, 120)), t.text()),
    ]));
    if let Some(root) = r.root.and_then(|s| m.sessions.get(&s)) {
        session_lines(app, root, 0, out);
    }
    match (&r.outcome, &r.answer) {
        (Some(o), Some(answer)) if o == "final" => {
            out.push(Line::from(""));
            out.push(Line::from(Span::styled(
                " FINAL ",
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.ok)
                    .add_modifier(Modifier::BOLD),
            )));
            out.extend(result_lines(&t, answer, false, "  "));
        }
        (Some(o), _) if o != "final" => {
            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::styled(
                    " NO FINAL ",
                    Style::default()
                        .fg(t.on_accent)
                        .bg(t.err)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {o}"), Style::default().fg(t.err)),
            ]));
        }
        _ => {}
    }
}

fn session_lines(app: &App, s: &SessionNode, depth: usize, out: &mut Vec<Line<'static>>) {
    let t = app.theme;
    let m = &app.model;
    let pad = "  ".repeat(depth + 1);
    let guide = if depth > 0 { "│ " } else { "" };
    let pad_text = format!("{pad}{guide}");
    if depth > 0 {
        out.push(Line::from(""));
        out.push(Line::from(vec![
            Span::styled(format!("{pad}↳ "), Style::default().fg(t.accent)),
            Span::styled(
                format!("{} {}", s.role, short(s.id)),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  d{}  ", s.depth), t.dim()),
            Span::styled(first_line(&s.task, 90), t.dim()),
        ]));
    }
    // Completed turns, with their statements' plans between the SQL and the
    // results.
    for turn in &s.turns_done {
        let usage = turn_usage(m, s, turn.n);
        out.push(Line::from(""));
        out.push(rule(
            &t,
            &pad_text,
            format!("turn {}", turn.n),
            usage,
            app.width,
        ));
        reply_lines(&t, &turn.reply, &pad_text, out);
        let plans = if app.show_plans {
            turn_plans(m, s, turn.n)
        } else {
            vec![]
        };
        for (i, r) in turn.results.iter().enumerate() {
            if let Some(Some(explain)) = plans.get(i) {
                out.push(Line::from(Span::styled(
                    format!("{pad_text}  ▾ plan"),
                    Style::default().fg(t.faint),
                )));
                for l in explain.lines() {
                    out.push(Line::from(Span::styled(
                        format!("{pad_text}    {l}"),
                        Style::default().fg(t.faint),
                    )));
                }
            }
            out.extend(result_lines(
                &t,
                &r.text,
                r.is_error,
                &format!("{pad_text}  "),
            ));
        }
    }
    // Children ran inside earlier turns, so they come before the live one.
    for child in &s.children {
        if let Some(c) = m.sessions.get(child) {
            session_lines(app, c, depth + 1, out);
        }
    }
    // The turn in progress: the reply streaming in, or the statements running.
    if s.outcome.is_none() {
        let n = s.turns_done.len() as u32 + 1;
        let live_turn = s
            .statement_ids
            .iter()
            .rev()
            .filter_map(|id| m.statements.get(id))
            .find(|st| st.sql.starts_with("-- turn"));
        let streaming = live_turn.and_then(|st| {
            st.call_ids
                .iter()
                .filter_map(|c| m.calls.get(c))
                .find(|c| !c.finished)
        });
        let executing: Vec<_> = s
            .statement_ids
            .iter()
            .filter_map(|id| m.statements.get(id))
            .filter(|st| !st.finished && !st.sql.starts_with("-- turn"))
            .collect();
        if streaming.is_some() || !executing.is_empty() || live_turn.is_some_and(|st| !st.finished)
        {
            out.push(Line::from(""));
            let state = if let Some(c) = streaming {
                if c.streaming.is_empty() {
                    "◐ thinking".to_string()
                } else {
                    "◐ writing".to_string()
                }
            } else if !executing.is_empty() {
                let calls: u64 = executing.iter().map(|st| st.calls).sum();
                format!("◐ executing · λ {calls}")
            } else {
                "◐".to_string()
            };
            out.push(rule(&t, &pad_text, format!("turn {n}"), state, app.width));
            if let Some(c) = streaming {
                reply_lines(&t, &c.streaming, &pad_text, out);
            }
            for st in executing {
                for l in st.sql.lines() {
                    out.push(Line::from(Span::styled(
                        format!("{pad_text}{l}"),
                        Style::default().fg(t.sql),
                    )));
                }
                out.push(Line::from(Span::styled(
                    format!(
                        "{pad_text}  ◐ running · {} call{} so far",
                        st.calls,
                        if st.calls == 1 { "" } else { "s" }
                    ),
                    Style::default().fg(t.ok).add_modifier(Modifier::ITALIC),
                )));
            }
        }
    }
    if let Some(o) = &s.outcome {
        if depth > 0 {
            out.push(Line::from(vec![
                Span::styled(
                    format!("{pad_text}■ "),
                    Style::default().fg(if o == "final" { t.ok } else { t.err }),
                ),
                Span::styled(
                    format!(
                        "{o} · {} turns · {} calls · {} tok · ${:.4}",
                        s.turns,
                        s.calls,
                        human(s.tokens),
                        s.dollars
                    ),
                    t.dim(),
                ),
            ]));
        } else {
            out.push(Line::from(Span::styled(
                format!(
                    "{pad_text}■ {o} · {} turns · {} calls · {} tok · ${:.4}",
                    s.turns,
                    s.calls,
                    human(s.tokens),
                    s.dollars
                ),
                t.dim(),
            )));
        }
    }
}

/// "k calls · $x" for a turn, from its pseudo-statement and its statements.
fn turn_usage(m: &Model, s: &SessionNode, n: u32) -> String {
    let marker = format!("-- turn {n}");
    let mut calls = 0u64;
    let mut dollars = 0.0f64;
    // Statements before any turn marker (a REPL session, a test) count as
    // turn 1.
    let mut in_turn = n == 1;
    for id in &s.statement_ids {
        let Some(st) = m.statements.get(id) else {
            continue;
        };
        if st.sql.starts_with("-- turn") {
            in_turn = st.sql == marker;
        }
        if in_turn {
            calls += st.calls;
            dollars += st.dollars;
        }
    }
    format!(
        "{calls} call{} · ${dollars:.4}",
        if calls == 1 { "" } else { "s" }
    )
}

/// The `EXPLAIN` of each statement of a turn, in order.
fn turn_plans(m: &Model, s: &SessionNode, n: u32) -> Vec<Option<String>> {
    let marker = format!("-- turn {n}");
    let mut plans = vec![];
    let mut in_turn = n == 1;
    for id in &s.statement_ids {
        let Some(st) = m.statements.get(id) else {
            continue;
        };
        if st.sql.starts_with("-- turn") {
            in_turn = st.sql == marker;
            continue;
        }
        if in_turn {
            plans.push(st.explain.clone());
        }
    }
    plans
}

/// A reply: prose dimmed, SQL inside fences in the SQL colour, fences hidden.
fn reply_lines(t: &Theme, reply: &str, pad: &str, out: &mut Vec<Line<'static>>) {
    let mut in_sql = false;
    for l in reply.lines() {
        let trimmed = l.trim_start();
        if trimmed.starts_with("```") {
            in_sql = !in_sql;
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let style = if in_sql {
            Style::default().fg(t.sql)
        } else {
            t.dim()
        };
        out.push(Line::from(Span::styled(format!("{pad}{l}"), style)));
    }
}

/// One rendered result: a table gets a bold header and dim rules, an error
/// goes red with its hint in yellow, the row count is dim.
pub fn result_lines(t: &Theme, text: &str, is_error: bool, pad: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut header_seen = false;
    for line in text.lines() {
        let tr = line.trim_start();
        let styled = if is_error {
            if let Some(rest) = tr.strip_prefix("error:") {
                Line::from(vec![
                    Span::styled(
                        format!("{pad}✗ error: "),
                        Style::default().fg(t.err).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(rest.trim().to_string(), Style::default().fg(t.err)),
                ])
            } else if let Some(rest) = tr.strip_prefix("hint:") {
                Line::from(vec![
                    Span::styled(format!("{pad}  hint: "), Style::default().fg(t.warn)),
                    Span::styled(rest.trim().to_string(), Style::default().fg(t.warn)),
                ])
            } else {
                Line::from(Span::styled(
                    format!("{pad}{line}"),
                    Style::default().fg(t.err),
                ))
            }
        } else if tr.contains("--+--")
            || (tr.starts_with('-') && tr.chars().all(|c| c == '-' || c == '+' || c == ' '))
        {
            header_seen = true;
            Line::from(Span::styled(
                format!("{pad}{line}"),
                Style::default().fg(t.border),
            ))
        } else if !header_seen && tr.contains('|') {
            Line::from(Span::styled(
                format!("{pad}{line}"),
                Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            ))
        } else if tr.ends_with(" rows") || tr.ends_with(" row") || tr == "FINAL" {
            Line::from(Span::styled(format!("{pad}{line}"), t.dim()))
        } else {
            Line::from(Span::styled(format!("{pad}{line}"), t.text()))
        };
        out.push(styled);
    }
    out
}

/// A box-drawn table sized to `width`, at most `max_rows` rows.
pub fn table_lines(
    t: &Theme,
    columns: &[String],
    rows: &[Vec<String>],
    max_cell: usize,
    width: usize,
    max_rows: usize,
) -> Vec<Line<'static>> {
    let ncols = columns.len().max(1);
    let shown = &rows[..rows.len().min(max_rows)];
    let mut widths: Vec<usize> = (0..ncols)
        .map(|i| {
            shown
                .iter()
                .map(|r| r.get(i).map(|v| v.chars().count()).unwrap_or(0))
                .max()
                .unwrap_or(0)
                .max(columns.get(i).map(|c| c.chars().count()).unwrap_or(0))
                .min(max_cell)
                .max(1)
        })
        .collect();
    let overhead = 3 * ncols + 1;
    while widths.iter().sum::<usize>() + overhead > width.max(overhead + ncols) {
        let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
            break;
        };
        if widths[i] <= 4 {
            break;
        }
        widths[i] -= 1;
    }
    let cell = |v: &str, w: usize| -> String {
        let n = v.chars().count();
        if n > w {
            format!(
                "{}…",
                v.chars().take(w.saturating_sub(1)).collect::<String>()
            )
        } else {
            format!("{v}{}", " ".repeat(w - n))
        }
    };
    let border = Style::default().fg(t.border);
    let rule = |l: &str, m: &str, r: &str| -> Line<'static> {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        Line::from(Span::styled(format!("{l}{}{r}", segs.join(m)), border))
    };
    let mut out = vec![rule("╭", "┬", "╮")];
    let mut header: Vec<Span<'static>> = vec![Span::styled("│", border)];
    for (i, w) in widths.iter().enumerate() {
        header.push(Span::styled(
            format!(
                " {} ",
                cell(columns.get(i).map(|s| s.as_str()).unwrap_or(""), *w)
            ),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ));
        header.push(Span::styled("│", border));
    }
    out.push(Line::from(header));
    out.push(rule("├", "┼", "┤"));
    for (ri, r) in shown.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = vec![Span::styled("│", border)];
        for (i, w) in widths.iter().enumerate() {
            let v = r.get(i).map(|s| s.as_str()).unwrap_or("");
            let style = if v == "NULL" {
                Style::default().fg(t.faint)
            } else if ri % 2 == 1 {
                Style::default().fg(t.fg).bg(t.sel_bg)
            } else {
                t.text()
            };
            spans.push(Span::styled(format!(" {} ", cell(v, *w)), style));
            spans.push(Span::styled("│", border));
        }
        out.push(Line::from(spans));
    }
    out.push(rule("╰", "┴", "╯"));
    if rows.len() > shown.len() {
        out.push(Line::from(Span::styled(
            format!("… {} more rows", rows.len() - shown.len()),
            t.dim(),
        )));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::{RunId, SessionId, StatementId};
    use kleene_daemon::{Cursor, ServerMessage, StatementOutput};
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
        app.apply(ServerMessage::TurnFinished {
            session: root,
            turn: 1,
            reply: "Counting.\n```sql\nSELECT COUNT(*) FROM ctx\n```".into(),
            sql: Some("SELECT COUNT(*) FROM ctx".into()),
            results: vec![StatementOutput {
                text: "count\n-----\n60\n1 row".into(),
                is_error: false,
                is_final: false,
            }],
        });
        app
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

    #[test]
    fn stream_shows_task_turn_sql_and_result() {
        let app = app_with_run();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("Find the bug"), "{text}");
        assert!(text.contains("turn 1"), "{text}");
        assert!(text.contains("SELECT COUNT(*) FROM ctx"), "{text}");
        assert!(text.contains("count"), "{text}");
        assert!(!text.contains("```"), "fences are hidden: {text}");
        assert!(text.contains("kleene"), "brand in the header: {text}");
    }

    #[test]
    fn welcome_block_and_help_render() {
        let mut app = App::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 34)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("/help"), "{text}");
        app.entries.push(Entry::Help);
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("Commands"), "{text}");
        assert!(text.contains("/trace"), "{text}");
    }

    #[test]
    fn plans_toggle_and_tables_fit() {
        let mut app = app_with_run();
        let lines = stream_lines(&app, 100);
        assert!(!lines.iter().any(|l| l.to_string().contains("scan ctx")));
        app.show_plans = true;
        let lines = stream_lines(&app, 100);
        assert!(lines.iter().any(|l| l.to_string().contains("scan ctx")));
        let t = Theme::default();
        let cols = vec!["name".to_string(), "n".to_string()];
        let rows = vec![
            vec!["a".to_string(), "1".to_string()],
            vec![
                "a much longer value than fits".to_string(),
                "NULL".to_string(),
            ],
        ];
        let table = table_lines(&t, &cols, &rows, 40, 24, 10);
        assert!(
            table.iter().all(|l| l.width() <= 24),
            "{:?}",
            table.iter().map(|l| l.width()).collect::<Vec<_>>()
        );
        assert_eq!(table.len(), 6, "rule, header, rule, two rows, rule");
    }

    #[test]
    fn wrapping_keeps_every_character() {
        let line = Line::from(vec![
            Span::raw("abcdefghij"),
            Span::styled(
                "klmnopqrst",
                Style::default().fg(ratatui::style::Color::Red),
            ),
        ]);
        let wrapped = wrap_lines(vec![line], 7);
        assert_eq!(wrapped.len(), 3);
        let joined: String = wrapped.iter().map(|l| l.to_string()).collect();
        assert_eq!(joined, "abcdefghijklmnopqrst");
    }
}
