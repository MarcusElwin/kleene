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

/// A turn's rule: `── turn 3 · <headline> ───── 1 call · $0.01`. The
/// headline is the first sentence of the model's prose, in normal text, cut
/// to fit; without one the rule is plain.
fn turn_rule(
    t: &Theme,
    prefix: &str,
    n: u32,
    headline: Option<&str>,
    right: String,
    width: usize,
) -> Line<'static> {
    let Some(head) = headline else {
        return rule(t, prefix, format!("turn {n}"), right, width);
    };
    let left = format!("── turn {n} · ");
    let fixed = prefix.chars().count() + left.chars().count() + right.chars().count() + 2 + 4;
    let room = width.saturating_sub(fixed + 1);
    let head: String = if head.chars().count() > room {
        format!(
            "{}…",
            head.chars()
                .take(room.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        head.to_string()
    };
    let fill = width
        .saturating_sub(fixed + head.chars().count() + 1)
        .max(4);
    Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(t.border)),
        Span::styled(left, Style::default().fg(t.faint)),
        Span::styled(head, t.text()),
        Span::styled(" ", Style::default()),
        Span::styled("─".repeat(fill), Style::default().fg(t.border)),
        Span::styled(format!(" {right} "), t.dim()),
    ])
}

/// The first sentence of the model's prose outside SQL fences, if any.
fn headline(reply: &str) -> Option<String> {
    let mut in_sql = false;
    for l in reply.lines() {
        let trimmed = l.trim();
        if trimmed.starts_with("```") {
            in_sql = !in_sql;
            continue;
        }
        if in_sql || trimmed.is_empty() {
            continue;
        }
        let text = trimmed.trim_start_matches(['#', '-', '*', ' ']);
        let mut end = text.len();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        for (i, (pos, c)) in chars.iter().enumerate() {
            if matches!(c, '.' | '!' | '?' | ':')
                && chars.get(i + 1).is_none_or(|(_, n)| n.is_whitespace())
            {
                end = pos + if *c == ':' { 0 } else { c.len_utf8() };
                break;
            }
        }
        let sentence = text[..end].trim().trim_end_matches('.');
        if !sentence.is_empty() {
            return Some(sentence.to_string());
        }
    }
    None
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
        (
            "Ctrl-P",
            "unfold each turn: the model's prose, EXPLAIN plans, the whole FINAL",
        ),
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
            let mut head = vec![Span::styled(
                " FINAL ",
                Style::default()
                    .fg(t.on_accent)
                    .bg(t.ok)
                    .add_modifier(Modifier::BOLD),
            )];
            if !r.answer_rows.is_empty() {
                let shown = r.answer_rows.len();
                let total = total_rows(answer, shown);
                let mut n = format!("  {total} row{}", if total == 1 { "" } else { "s" });
                if total > shown {
                    n.push_str(&format!(", first {shown} shown"));
                }
                head.push(Span::styled(n, t.dim()));
            }
            out.push(Line::from(head));
            if r.answer_rows.is_empty() {
                // An older daemon sent only the text the model saw.
                out.extend(result_lines(&t, answer, false, "  "));
            } else {
                out.extend(final_lines(
                    &t,
                    &r.answer_columns,
                    &r.answer_rows,
                    app.width,
                    "  ",
                ));
            }
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
        let head = if app.show_plans {
            None
        } else {
            headline(&turn.reply)
        };
        out.push(turn_rule(
            &t,
            &pad_text,
            turn.n,
            head.as_deref(),
            usage,
            app.width,
        ));
        reply_lines(&t, &turn.reply, &pad_text, !app.show_plans, out);
        let plans = if app.show_plans {
            turn_plans(m, s, turn.n)
        } else {
            vec![]
        };
        for (i, r) in turn.results.iter().enumerate() {
            // The root's FINAL is laid out for a person at the end of the
            // run, so here it folds to one line; Ctrl-P unfolds the table
            // the model saw.
            if r.is_final && depth == 0 && !app.show_plans {
                if let Some(run) = m.runs.get(&s.run) {
                    if run.outcome.as_deref() == Some("final") && run.answer.is_some() {
                        let n = if run.answer_rows.is_empty() {
                            r.text.lines().count().saturating_sub(2)
                        } else {
                            total_rows(run.answer.as_deref().unwrap_or(""), run.answer_rows.len())
                        };
                        out.push(Line::from(vec![
                            Span::styled(format!("{pad_text}  FINAL"), t.dim()),
                            Span::styled(
                                format!(
                                    " · {n} row{} · the answer is below",
                                    if n == 1 { "" } else { "s" }
                                ),
                                Style::default().fg(t.faint),
                            ),
                        ]));
                        continue;
                    }
                }
            }
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
    // The plan as of the newest turn that had one.
    if let Some(plan) = s
        .turns_done
        .iter()
        .rev()
        .map(|t| &t.plan)
        .find(|p| !p.is_empty())
    {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!("{pad_text}plan"),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )));
        for (step, status) in plan {
            let (mark, style) = match status.trim().to_ascii_lowercase().as_str() {
                "done" => ("✓", Style::default().fg(t.ok)),
                "doing" => ("◐", Style::default().fg(t.accent)),
                _ => ("○", t.dim()),
            };
            out.push(Line::from(vec![
                Span::styled(format!("{pad_text}  {mark} "), style),
                Span::styled(step.clone(), style),
                Span::styled(format!("  {status}"), t.dim()),
            ]));
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
                reply_lines(&t, &c.streaming, &pad_text, false, out);
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
/// Folded, the prose is left out (its first sentence is on the turn's rule)
/// and a `FINAL` statement shows only its first line, since the answer is
/// laid out at the end of the run.
fn reply_lines(t: &Theme, reply: &str, pad: &str, fold: bool, out: &mut Vec<Line<'static>>) {
    let mut in_sql = false;
    let mut in_final = false;
    for l in reply.lines() {
        let trimmed = l.trim_start();
        if trimmed.starts_with("```") {
            in_sql = !in_sql;
            in_final = false;
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if !in_sql {
            if !fold {
                out.push(Line::from(Span::styled(format!("{pad}{l}"), t.dim())));
            }
            continue;
        }
        let sql = Style::default().fg(t.sql);
        if fold && in_final {
            if trimmed.trim_end().ends_with(';') {
                in_final = false;
            }
            continue;
        }
        if fold && trimmed.starts_with("FINAL") && !trimmed.trim_end().ends_with(';') {
            in_final = true;
            out.push(Line::from(vec![
                Span::styled(format!("{pad}{l}"), sql),
                Span::styled(" …", Style::default().fg(t.faint)),
            ]));
            continue;
        }
        out.push(Line::from(Span::styled(format!("{pad}{l}"), sql)));
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

/// The answer's row count: `shown` unless the rendered text ends with the
/// `... k more rows (n total)` line the renderer adds past its cap.
fn total_rows(rendered: &str, shown: usize) -> usize {
    rendered
        .lines()
        .last()
        .and_then(|l| l.strip_prefix("... "))
        .and_then(|l| l.rsplit_once('('))
        .and_then(|(_, tail)| tail.strip_suffix(" total)"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(shown)
}

/// Cells up to this wide, in a table that fits, keep the grid; anything
/// longer reads as cards.
const GRID_CELL: usize = 32;

/// Long text wraps at this width even on a wide terminal, so a paragraph
/// stays a readable measure.
const MEASURE: usize = 100;

/// The FINAL relation laid out for a person, never cutting a cell short.
///
/// A table of short cells that fits the width stays a grid. Otherwise each
/// row is a card: a single-column relation is a list (one value alone is
/// plain text), and a wider one shows its first column as the title when
/// that is short, then one `name  value` line per remaining column with
/// the value word-wrapped under itself.
pub fn final_lines(
    t: &Theme,
    columns: &[String],
    rows: &[Vec<String>],
    width: usize,
    pad: &str,
) -> Vec<Line<'static>> {
    let pad_w = pad.chars().count();
    let inner = width.saturating_sub(pad_w).max(24);
    let ncols = columns.len().max(1);
    let cell_w = |v: &str| v.chars().count();
    let longest = rows
        .iter()
        .flat_map(|r| r.iter().map(|c| cell_w(c)))
        .chain(columns.iter().map(|c| cell_w(c)))
        .max()
        .unwrap_or(0);
    let multiline = rows.iter().flatten().any(|c| c.contains('\n'));
    if ncols >= 2 && !multiline && longest <= GRID_CELL {
        let natural: usize = (0..ncols)
            .map(|i| {
                rows.iter()
                    .map(|r| r.get(i).map(|c| cell_w(c)).unwrap_or(0))
                    .chain(columns.get(i).map(|c| cell_w(c)))
                    .max()
                    .unwrap_or(1)
            })
            .sum::<usize>()
            + 3 * ncols
            + 1;
        if natural <= inner {
            return table_lines(t, columns, rows, GRID_CELL, inner, rows.len())
                .into_iter()
                .map(|l| {
                    let mut spans = vec![Span::raw(pad.to_string())];
                    spans.extend(l.spans);
                    Line::from(spans)
                })
                .collect();
        }
    }
    let measure = inner.min(MEASURE);
    let value_style = |v: &str| {
        if v == "NULL" {
            Style::default().fg(t.faint)
        } else {
            t.text()
        }
    };
    let mut out = Vec::new();
    if ncols == 1 {
        let one = rows.len() == 1;
        for r in rows {
            let v = r.first().map(|s| s.as_str()).unwrap_or("");
            let (first, rest) = if one { ("", "") } else { ("• ", "  ") };
            for (i, l) in wrap_words(v, measure.saturating_sub(2))
                .into_iter()
                .enumerate()
            {
                let lead = if i == 0 { first } else { rest };
                out.push(Line::from(vec![
                    Span::styled(format!("{pad}{lead}"), Style::default().fg(t.accent)),
                    Span::styled(l, value_style(v)),
                ]));
            }
        }
        return out;
    }
    let titled = rows.iter().all(|r| {
        r.first()
            .is_some_and(|c| cell_w(c) <= 48 && !c.contains('\n') && !c.is_empty())
    });
    let fields: Vec<usize> = (if titled { 1 } else { 0 }..ncols).collect();
    let name_w = fields
        .iter()
        .map(|&i| columns.get(i).map(|c| cell_w(c)).unwrap_or(0))
        .max()
        .unwrap_or(0);
    let lead = if titled { "  " } else { "" };
    let value_w = measure.saturating_sub(lead.len() + name_w + 2).max(16);
    for (ri, r) in rows.iter().enumerate() {
        if ri > 0 {
            out.push(Line::from(""));
        }
        if titled {
            out.push(Line::from(vec![
                Span::styled(format!("{pad}▍ "), Style::default().fg(t.accent)),
                Span::styled(
                    r.first().cloned().unwrap_or_default(),
                    Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        for &i in &fields {
            let name = columns.get(i).map(|s| s.as_str()).unwrap_or("");
            let v = r.get(i).map(|s| s.as_str()).unwrap_or("");
            for (li, l) in wrap_words(v, value_w).into_iter().enumerate() {
                let label = if li == 0 {
                    format!("{pad}{lead}{name:>name_w$}  ")
                } else {
                    format!("{pad}{lead}{:>name_w$}  ", "")
                };
                out.push(Line::from(vec![
                    Span::styled(label, Style::default().fg(t.muted)),
                    Span::styled(l, value_style(v)),
                ]));
            }
        }
    }
    out
}

/// Word-wrap `text` to `width` columns, keeping its own line breaks and
/// the spacing between words (a cell may hold code); a word longer than
/// the width is split. Always at least one line.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut used = 0usize;
        let chars: Vec<char> = para.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            // The whitespace run before the next word, kept when the word
            // continues the line and dropped at a line break.
            let gap_start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let gap = chars.len().min(i) - gap_start;
            let word_start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            let mut word = &chars[word_start..i];
            if word.is_empty() {
                continue;
            }
            while !word.is_empty() {
                let n = word.len();
                if used > 0 && used + gap + n <= width {
                    line.extend(&chars[gap_start..word_start]);
                    line.extend(word);
                    used += gap + n;
                    break;
                } else if used == 0 && n <= width {
                    line.extend(word);
                    used = n;
                    break;
                } else if used > 0 {
                    out.push(std::mem::take(&mut line));
                    used = 0;
                } else {
                    line.extend(&word[..width]);
                    out.push(std::mem::take(&mut line));
                    word = &word[width..];
                }
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
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
            plan: vec![],
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

    fn finish_with_final(app: &mut App, columns: &[&str], rows: &[&[&str]]) {
        let run = app.model.run_order[0];
        let root = app.model.roots()[0];
        let text = format!(
            "FINAL\n{}\n{}\n{}",
            columns.join(" | "),
            columns
                .iter()
                .map(|_| "---")
                .collect::<Vec<_>>()
                .join("-+-"),
            rows.iter()
                .map(|r| r
                    .iter()
                    .map(|c| c.chars().take(57).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" | "))
                .collect::<Vec<_>>()
                .join("\n")
        );
        app.apply(ServerMessage::TurnFinished {
            session: root,
            turn: 2,
            reply: "Done.".into(),
            sql: Some("FINAL FROM (SELECT * FROM answer)".into()),
            results: vec![StatementOutput {
                text: text.clone(),
                is_error: false,
                is_final: true,
            }],
            plan: vec![],
        });
        app.apply(ServerMessage::RunFinished {
            run,
            outcome: "final".into(),
            answer: Some(text),
            answer_columns: columns.iter().map(|c| c.to_string()).collect(),
            answer_rows: rows
                .iter()
                .map(|r| r.iter().map(|c| c.to_string()).collect())
                .collect(),
        });
    }

    const LONG: &str = "Query session tables with SQL (joins, aggregates, recursive CTEs), and add yes/no, scoring or JSON judgements by calling a language model on each row";

    #[test]
    fn final_cards_keep_whole_cells_and_fold_the_turn_copy() {
        let mut app = app_with_run();
        finish_with_final(
            &mut app,
            &["capability", "how"],
            &[
                &["Answer questions over data", LONG],
                &[
                    "Read and search files",
                    "List, grep and read workspace files",
                ],
            ],
        );
        let lines = stream_lines(&app, 100);
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let joined = text.join("\n");
        assert!(text.iter().all(|l| l.chars().count() <= 100), "{joined}");
        // Every character of the long cell survives, across wrapped lines,
        // and the rows appear once: the turn's own FINAL table folds to one
        // line with the row count.
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let squashed = squash(&joined);
        assert_eq!(squashed.matches(&squash(LONG)).count(), 1, "{joined}");
        assert!(!joined.contains("..."), "no truncation: {joined}");
        assert!(joined.contains("▍ Answer questions over data"), "{joined}");
        assert!(
            joined.contains("FINAL · 2 rows · the answer is below"),
            "{joined}"
        );
        // Ctrl-P brings the model's table back.
        app.show_plans = true;
        let joined: String = stream_lines(&app, 100)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("---+---"), "{joined}");
        assert!(!joined.contains("the answer is below"), "{joined}");
    }

    #[test]
    fn prose_folds_to_a_headline_and_final_sql_to_one_line() {
        let mut app = app_with_run();
        let root = app.model.roots()[0];
        let run = app.model.run_order[0];
        app.apply(ServerMessage::TurnFinished {
            session: root,
            turn: 2,
            reply: "The function never strips the hyphens. I'll patch it and rerun.\n\n```sql\nCALL patch('a.py', $$x$$, $$y$$);\nFINAL FROM (SELECT 'a.py' AS file,\n  'a very long explanation' AS why);\n```".into(),
            sql: None,
            results: vec![StatementOutput {
                text: "FINAL\nfile | why\n-----+----\na.py | a very long explanation".into(),
                is_error: false,
                is_final: true,
            }],
            plan: vec![],
        });
        app.apply(ServerMessage::RunFinished {
            run,
            outcome: "final".into(),
            answer: Some("file | why\n-----+----\na.py | a very long explanation\n".into()),
            answer_columns: vec!["file".into(), "why".into()],
            answer_rows: vec![vec!["a.py".into(), "a very long explanation".into()]],
        });
        let joined = |app: &App| {
            stream_lines(app, 100)
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let text = joined(&app);
        assert!(
            text.contains("── turn 2 · The function never strips the hyphens ─"),
            "{text}"
        );
        assert!(
            !text.contains("I'll patch it"),
            "rest of the prose folded: {text}"
        );
        assert!(text.contains("CALL patch('a.py', $$x$$, $$y$$);"), "{text}");
        assert!(
            text.contains("FINAL FROM (SELECT 'a.py' AS file, …"),
            "{text}"
        );
        assert_eq!(text.matches("a very long explanation").count(), 1, "{text}");
        app.show_plans = true;
        let text = joined(&app);
        assert!(text.contains("── turn 2 ─"), "{text}");
        assert!(text.contains("I'll patch it"), "{text}");
        assert!(
            text.contains("'a very long explanation' AS why);"),
            "{text}"
        );
        assert_eq!(headline("```sql\nSELECT 1\n```"), None);
        assert_eq!(
            headline("Counting: there are many.\n").as_deref(),
            Some("Counting")
        );
        assert_eq!(headline("Done!").as_deref(), Some("Done!"));
        assert_eq!(
            headline("v1.2 is out. Next.").as_deref(),
            Some("v1.2 is out")
        );
        let line = turn_rule(
            &Theme::default(),
            "",
            7,
            Some(&"x".repeat(200)),
            "1 call".into(),
            60,
        );
        assert!(line.width() <= 60, "{}", line.width());
        assert!(line.to_string().contains('…'));
    }

    #[test]
    fn final_grid_for_short_cells_and_text_for_one_value() {
        let t = Theme::default();
        let cols = ["project".to_string(), "total_hours".to_string()];
        let rows = vec![
            vec!["Osprey".to_string(), "212.5".to_string()],
            vec!["Heron".to_string(), "NULL".to_string()],
        ];
        let grid = final_lines(&t, &cols, &rows, 100, "  ");
        let joined: String = grid
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("│ Osprey"), "{joined}");
        assert_eq!(
            grid.len(),
            6,
            "rule, header, rule, two rows, rule: {joined}"
        );
        let one = final_lines(
            &t,
            &["answer".to_string()],
            &[vec!["42".to_string()]],
            100,
            "  ",
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].to_string(), "  42");
        let list = final_lines(
            &t,
            &["name".to_string()],
            &[vec!["Osprey".to_string()], vec!["Heron".to_string()]],
            100,
            "  ",
        );
        assert_eq!(list[0].to_string(), "  • Osprey");
        assert_eq!(list[1].to_string(), "  • Heron");
        // Rows whose first cell is itself long get no title, only fields.
        let untitled = final_lines(
            &t,
            &["a".to_string(), "b".to_string()],
            &[vec![LONG.to_string(), "x".to_string()]],
            60,
            "",
        );
        let joined: String = untitled
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains('▍'), "{joined}");
        assert!(untitled.iter().all(|l| l.width() <= 60), "{joined}");
        assert!(joined.starts_with("a  Query"), "{joined}");
        assert!(joined.ends_with("b  x"), "{joined}");
    }

    #[test]
    fn word_wrap_breaks_between_words_and_splits_long_words() {
        assert_eq!(wrap_words("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap_words("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_words("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap_words("x = 1;  y = 2", 20), ["x = 1;  y = 2"]);
        assert_eq!(wrap_words("  lead", 10), ["lead"]);
        assert_eq!(wrap_words("ab  cd", 4), ["ab", "cd"]);
        assert_eq!(wrap_words("", 10), [""]);
        assert_eq!(total_rows("x\n-\n1\n... 3 more rows (53 total)\n", 50), 53);
        assert_eq!(total_rows("x\n-\n1\n", 1), 1);
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
