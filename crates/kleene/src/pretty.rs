//! Terminal output for `kleene run`: the model's reply streamed as it is
//! written, a spinner while it thinks and while its SQL runs, coloured
//! results, and a boxed final relation. Colour and spinners switch off on
//! their own when stderr is not a terminal, so piped output stays plain.

use console::{style, Style, Term};
use indicatif::{ProgressBar, ProgressStyle};
use kleene_core::{Batch, BudgetUsage, CallId, SessionId};
use kleene_harness::{Observer, Outcome, SessionMeta, Turn};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Prints sessions as they run.
pub struct Printer {
    quiet: bool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Turn counter per session, for the rule above each turn.
    turns: HashMap<SessionId, u32>,
    /// The spinner shown while the root session thinks or executes.
    spinner: Option<ProgressBar>,
    /// Model calls made by the root session's statements in this turn.
    statement_calls: u64,
    /// Streaming state of the root session's current reply.
    stream: Option<Stream>,
}

/// Fence-aware incremental printing of one reply.
#[derive(Default)]
struct Stream {
    /// Text of the current (unterminated) line.
    line: String,
    /// How many chars of `line` have been printed already.
    printed: usize,
    /// Inside a ```sql fence.
    in_fence: bool,
    /// Anything at all was written.
    wrote: bool,
}

const SPINNER_TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Printer {
    /// `quiet` prints nothing but the final relation.
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            state: Mutex::new(State::default()),
        }
    }

    fn spinner(msg: String) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner())
                .tick_strings(SPINNER_TICKS),
        );
        pb.set_message(msg);
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }

    fn stop_spinner(state: &mut State) {
        if let Some(pb) = state.spinner.take() {
            pb.finish_and_clear();
        }
    }

    /// Write streamed text, colouring SQL inside fences and hiding the fence
    /// lines themselves. Complete lines are styled as a whole; the current
    /// partial line is printed as it grows unless it might be a fence.
    fn write_delta(st: &mut Stream, text: &str) {
        let term = Term::stderr();
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut st.line);
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    // Fence: nothing printed for it; flip the mode. If part of
                    // it was already printed (should not happen, fences are
                    // held), clear that.
                    if st.printed > 0 {
                        let _ = term.clear_line();
                    }
                    st.in_fence = !st.in_fence;
                } else {
                    let rest: String = line.chars().skip(st.printed).collect();
                    let _ = term.write_str(&Self::styled(&rest, st.in_fence));
                    let _ = term.write_line("");
                    st.wrote = true;
                }
                st.printed = 0;
            } else {
                st.line.push(ch);
            }
        }
        // Print the partial line unless it could still turn into a fence.
        let trimmed = st.line.trim_start();
        let may_be_fence = trimmed.is_empty() || trimmed.starts_with('`');
        if !may_be_fence {
            let rest: String = st.line.chars().skip(st.printed).collect();
            if !rest.is_empty() {
                let _ = term.write_str(&Self::styled(&rest, st.in_fence));
                st.printed = st.line.chars().count();
                st.wrote = true;
            }
        }
    }

    fn flush_stream(st: &mut Stream) {
        if !st.line.is_empty() && !st.line.trim().starts_with("```") {
            let rest: String = st.line.chars().skip(st.printed).collect();
            let term = Term::stderr();
            let _ = term.write_str(&Self::styled(&rest, st.in_fence));
            let _ = term.write_line("");
            st.wrote = true;
        }
        st.line.clear();
        st.printed = 0;
    }

    fn styled(text: &str, sql: bool) -> String {
        if sql {
            style(text).cyan().to_string()
        } else {
            style(text).dim().to_string()
        }
    }
}

/// The tail of an id: UUID v7 starts with a timestamp, so the head is the
/// same for every id minted in the same run.
fn short(id: &impl ToString) -> String {
    let s = id.to_string();
    s[s.len().saturating_sub(6)..].to_string()
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().next().unwrap_or_default().trim();
    if l.chars().count() > max {
        format!("{}…", l.chars().take(max).collect::<String>())
    } else {
        l.to_string()
    }
}

fn indent(depth: u32) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!("{}", style("│ ".repeat(depth as usize)).dim())
    }
}

/// Colour one rendered result: a table gets a bold header and dim rules,
/// errors go red with the hint in yellow.
fn colour_result(text: &str, is_error: bool, prefix: &str) -> String {
    let mut out = String::new();
    let mut header_seen = false;
    for line in text.lines() {
        let t = line.trim_start();
        let styled = if is_error {
            if let Some(rest) = t.strip_prefix("error:") {
                format!(
                    "{} {}",
                    style("✗ error:").red().bold(),
                    style(rest.trim()).red()
                )
            } else if let Some(rest) = t.strip_prefix("hint:") {
                format!(
                    "{} {}",
                    style("  hint:").yellow(),
                    style(rest.trim()).yellow()
                )
            } else {
                style(line).red().to_string()
            }
        } else if t.contains("--+--")
            || (t.starts_with('-') && t.chars().all(|c| c == '-' || c == '+' || c == ' '))
        {
            header_seen = true;
            style(line).dim().to_string()
        } else if !header_seen && t.contains('|') {
            style(line).bold().to_string()
        } else if t.ends_with(" rows") || t.ends_with(" row") || t.starts_with("FINAL") {
            style(line).dim().to_string()
        } else {
            line.to_string()
        };
        out.push_str(prefix);
        out.push_str(&styled);
        out.push('\n');
    }
    out
}

impl Observer for Printer {
    fn session_started(&self, meta: &SessionMeta, task: &str) {
        if self.quiet {
            return;
        }
        let term = Term::stderr();
        if meta.depth == 0 {
            let _ = term.write_line(&format!(
                "{} {} {}",
                style(" ◆ kleene ").black().on_cyan().bold(),
                style(format!("run {}", short(&meta.run))).dim(),
                style(first_line(task, 100)).bold()
            ));
        } else {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Self::stop_spinner(&mut state);
            let _ = term.write_line(&format!(
                "{}{} {} {} {}",
                indent(meta.depth),
                style("↳").cyan(),
                style(format!("{} {}", meta.role.name, short(&meta.id))).cyan(),
                style(format!("d{}", meta.depth)).dim(),
                style(first_line(task, 80)).dim()
            ));
        }
    }

    fn call_started(&self, meta: &SessionMeta, _call: CallId, alias: &str, turn: bool) {
        if self.quiet || meta.depth > 0 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if turn {
            let n = state.turns.entry(meta.id).or_insert(0);
            *n += 1;
            let n = *n;
            Self::stop_spinner(&mut state);
            let _ = Term::stderr().write_line(&format!(
                "{}",
                style(format!("── turn {n} ─────────────────────────────")).dim()
            ));
            state.statement_calls = 0;
            state.stream = Some(Stream::default());
            state.spinner = Some(Self::spinner(format!(
                "{} {}",
                style("thinking").dim(),
                style(alias).dim()
            )));
        } else {
            state.statement_calls += 1;
            let n = state.statement_calls;
            if let Some(pb) = &state.spinner {
                pb.set_message(format!(
                    "{} {}",
                    style("executing").dim(),
                    style(format!("λ {n} model call{}", if n == 1 { "" } else { "s" })).dim()
                ));
            }
        }
    }

    fn call_delta(&self, meta: &SessionMeta, _call: CallId, text: &str) {
        if self.quiet || meta.depth > 0 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::stop_spinner(&mut state);
        let mut st = state.stream.take().unwrap_or_default();
        Self::write_delta(&mut st, text);
        state.stream = Some(st);
    }

    fn call_finished(
        &self,
        meta: &SessionMeta,
        _call: CallId,
        _memo_hit: bool,
        error: Option<&str>,
    ) {
        if self.quiet || meta.depth > 0 {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut st) = state.stream.take() {
            // The reply is complete: flush it and switch to the execution spinner.
            Self::stop_spinner(&mut state);
            Self::flush_stream(&mut st);
            if let Some(e) = error {
                let _ = Term::stderr().write_line(&format!(
                    "{} {}",
                    style("✗").red().bold(),
                    style(e).red()
                ));
            } else {
                state.spinner = Some(Self::spinner(style("executing").dim().to_string()));
            }
        }
    }

    fn turn(&self, meta: &SessionMeta, turn: &Turn) {
        if self.quiet {
            return;
        }
        let term = Term::stderr();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::stop_spinner(&mut state);
        let pad = indent(meta.depth);
        if meta.depth > 0 {
            // Children are summarised: their SQL, then errors and the footer.
            let _ = term.write_line(&format!("{pad}{}", style(format!("turn {}", turn.n)).dim()));
            match &turn.sql {
                Some(sql) => {
                    for l in sql.lines() {
                        let _ = term.write_line(&format!("{pad}  {}", style(l).cyan().dim()));
                    }
                }
                None => {
                    let _ = term.write_line(&format!(
                        "{pad}  {}",
                        style(format!("(no SQL) {}", first_line(&turn.reply, 80))).dim()
                    ));
                }
            }
            for r in turn.results.iter().filter(|r| r.is_error) {
                let _ = term.write_str(&colour_result(&r.text, true, &format!("{pad}  ")));
            }
        } else {
            if turn.sql.is_none() {
                let _ = term.write_line(&format!(
                    "{} {}",
                    style("⚠").yellow(),
                    style("no SQL in the reply; the model was nudged").yellow()
                ));
            }
            for r in &turn.results {
                let _ = term.write_str(&colour_result(&r.text, r.is_error, "  "));
                let _ = term.write_line("");
            }
        }
        if let Some(footer) = turn
            .feedback
            .lines()
            .find(|l| l.trim_start().starts_with("turn "))
        {
            let _ = term.write_line(&format!("{pad}  {}", style(footer.trim()).dim()));
        }
    }

    fn session_finished(&self, meta: &SessionMeta, outcome: &Outcome, usage: &BudgetUsage) {
        if self.quiet {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::stop_spinner(&mut state);
        let ok = matches!(outcome, Outcome::Final { .. });
        let mark = if ok {
            style("■").green()
        } else {
            style("■").red()
        };
        let tag = if ok {
            style(outcome.tag()).green().bold()
        } else {
            style(outcome.tag()).red().bold()
        };
        let _ = Term::stderr().write_line(&format!(
            "{}{mark} {} {tag} {}",
            indent(meta.depth),
            style(format!("session {}", short(&meta.id))).dim(),
            style(format!(
                "· {} calls · {} tok · ${:.4} · {:.1}s",
                usage.calls,
                human(usage.tokens),
                usage.dollars,
                usage.wall.as_secs_f64()
            ))
            .dim()
        ));
    }
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

/// A relation as a box-drawn table sized to the terminal.
pub fn table(batch: &Batch, max_rows: usize) -> String {
    let width = Term::stdout().size().1.max(40) as usize;
    let names = batch.schema.names();
    let rows: Vec<Vec<String>> = batch
        .rows
        .iter()
        .take(max_rows)
        .map(|r| r.iter().map(kleene_core::Value::render).collect())
        .collect();
    let ncols = names.len().max(1);
    // Natural widths, then shrink the widest columns until the table fits.
    let mut widths: Vec<usize> = (0..ncols)
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map(|v| v.chars().count()).unwrap_or(0))
                .max()
                .unwrap_or(0)
                .max(names.get(i).map(|n| n.chars().count()).unwrap_or(0))
                .max(1)
        })
        .collect();
    let overhead = 3 * ncols + 1;
    while widths.iter().sum::<usize>() + overhead > width {
        let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
            break;
        };
        if widths[i] <= 6 {
            break;
        }
        widths[i] -= 1;
    }
    let cell = |v: &str, w: usize| -> String {
        let n = v.chars().count();
        if n > w {
            let keep: String = v.chars().take(w.saturating_sub(1)).collect();
            format!("{keep}…")
        } else {
            format!("{v}{}", " ".repeat(w - n))
        }
    };
    let rule = |l: &str, m: &str, r: &str| -> String {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        format!("{l}{}{r}", segs.join(m))
    };
    let dim = Style::new().dim();
    let mut out = String::new();
    out.push_str(&dim.apply_to(rule("╭", "┬", "╮")).to_string());
    out.push('\n');
    let header: Vec<String> = (0..ncols)
        .map(|i| {
            style(cell(names.get(i).copied().unwrap_or(""), widths[i]))
                .bold()
                .to_string()
        })
        .collect();
    out.push_str(&format!(
        "{} {} {}\n",
        dim.apply_to("│"),
        header.join(&format!(" {} ", dim.apply_to("│"))),
        dim.apply_to("│")
    ));
    out.push_str(&dim.apply_to(rule("├", "┼", "┤")).to_string());
    out.push('\n');
    for r in &rows {
        let cells: Vec<String> = (0..ncols)
            .map(|i| {
                let v = r.get(i).map(|s| s.as_str()).unwrap_or("");
                if v == "NULL" {
                    dim.apply_to(cell(v, widths[i])).to_string()
                } else {
                    cell(v, widths[i])
                }
            })
            .collect();
        out.push_str(&format!(
            "{} {} {}\n",
            dim.apply_to("│"),
            cells.join(&format!(" {} ", dim.apply_to("│"))),
            dim.apply_to("│")
        ));
    }
    out.push_str(&dim.apply_to(rule("╰", "┴", "╯")).to_string());
    out.push('\n');
    if batch.rows.len() > rows.len() {
        out.push_str(&format!(
            "{}\n",
            dim.apply_to(format!("… {} more rows", batch.rows.len() - rows.len()))
        ));
    }
    out
}

/// The final report: a titled panel with the answer, or the reason there is
/// none. Returns the process exit code.
pub fn report(report: &kleene_harness::RunReport) -> i32 {
    let term = Term::stdout();
    let _ = term.write_line(&format!(
        "{} {}",
        style("run").dim(),
        style(report.run.to_string()).dim()
    ));
    match &report.root.outcome {
        Outcome::Final { answer } => {
            let _ = term.write_line(&format!(
                "{} {}",
                style(" FINAL ").black().on_green().bold(),
                style(format!(
                    "{} row{} · {} turns · {} calls · {} tok · ${:.4}",
                    answer.len(),
                    if answer.len() == 1 { "" } else { "s" },
                    report.root.turns,
                    report.root.usage.calls,
                    human(report.root.usage.tokens),
                    report.root.usage.dollars
                ))
                .dim()
            ));
            let _ = term.write_str(&table(answer, 200));
            0
        }
        other => {
            let reason = match other {
                Outcome::BudgetExhausted { detail } => format!("budget exhausted ({detail})"),
                Outcome::TurnsExhausted => format!(
                    "turn cap reached; `kleene resume {}` continues it",
                    report.run
                ),
                Outcome::Failed { error } => format!("failed: {error}"),
                Outcome::Cancelled => "cancelled".into(),
                Outcome::Final { .. } => unreachable!(),
            };
            let _ = term.write_line(&format!(
                "{} {} {}",
                style(" NO FINAL ").black().on_red().bold(),
                style(reason).red(),
                style(format!(
                    "· {} turns · {} calls · {} tok · ${:.4}",
                    report.root.turns,
                    report.root.usage.calls,
                    human(report.root.usage.tokens),
                    report.root.usage.dollars
                ))
                .dim()
            ));
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::{DataType, Field, Schema, Value};

    #[test]
    fn table_fits_and_truncates() {
        console::set_colors_enabled(false);
        let schema = Schema::new(vec![
            Field::new("name", DataType::Text),
            Field::new("n", DataType::Int),
        ]);
        let batch = Batch::try_new(
            std::sync::Arc::new(schema),
            vec![
                vec![Value::from("a"), Value::Int(1)],
                vec![
                    Value::from("a much longer value than the column"),
                    Value::Null,
                ],
            ],
        )
        .unwrap();
        let t = table(&batch, 200);
        assert!(t.starts_with('╭'), "{t}");
        assert!(t.contains("│ name"), "{t}");
        assert!(t.contains("NULL"), "{t}");
        assert!(t.lines().all(|l| l.chars().count() <= 200));
    }

    #[test]
    fn results_are_coloured_by_kind() {
        console::set_colors_enabled(false);
        let table = colour_result("a | b\n--+--\n1 | 2\n1 row", false, "  ");
        assert_eq!(table.lines().count(), 4);
        let err = colour_result("error: unsupported\nhint: plain arguments", true, "");
        assert!(err.contains("✗ error: unsupported"), "{err}");
        assert!(err.contains("hint: plain arguments"), "{err}");
    }

    #[test]
    fn streaming_hides_fences_and_tracks_mode() {
        let mut st = Stream::default();
        Printer::write_delta(&mut st, "Let me look.\n``");
        assert!(!st.in_fence);
        assert_eq!(st.line, "``", "a possible fence is held back");
        assert_eq!(st.printed, 0);
        Printer::write_delta(&mut st, "`sql\nSELECT 1");
        assert!(st.in_fence);
        assert_eq!(st.line, "SELECT 1");
        assert_eq!(st.printed, 8, "sql is printed as it arrives");
        Printer::write_delta(&mut st, ";\n```\n");
        assert!(!st.in_fence);
        assert!(st.line.is_empty());
    }
}
