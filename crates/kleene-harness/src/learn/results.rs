//! Results across models: the README's summary table and the per-pack
//! quality-against-cost (Pareto) plots, built from `evals` CSV files rather
//! than from a store, so results recorded by different runs, machines and
//! models (`plots/evals.csv`, `plots/haiku-2026-10-01/evals.csv`, ...) can
//! be put side by side without merging DuckDB files. Every row names its
//! model; a (pack, mode) a model has not been run on is left blank in the
//! table and absent from the plot, so adding a model is adding its CSV.

use super::plots::{render, Chart, Series};
use crate::HarnessError;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// One eval row, as `bench csv` writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalRecord {
    /// Pack name.
    pub pack: String,
    /// `learning`, `frozen` or `plain`.
    pub mode: String,
    /// The solver model.
    pub model: String,
    /// The oracle's verdict.
    pub solved: bool,
    /// Model calls.
    pub calls: f64,
    /// Tokens.
    pub tokens: f64,
    /// Dollars.
    pub dollars: f64,
    /// Deepest child session.
    pub depth: f64,
    /// Wall-clock milliseconds.
    pub wall_ms: f64,
}

/// Parse an `evals` CSV. The header names the columns, so the order does
/// not matter; `pack`, `mode`, `model`, `solved` and `dollars` are required
/// and a missing or empty `model` is an error, because a row without a
/// model cannot be placed in the table.
pub fn parse_csv(text: &str, source: &str) -> Result<Vec<EvalRecord>, HarnessError> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| HarnessError::Config(format!("{source}: empty CSV")))?;
    let columns: Vec<String> = split_csv(header);
    let col = |name: &str| -> Result<usize, HarnessError> {
        columns.iter().position(|c| c == name).ok_or_else(|| {
            HarnessError::Config(format!(
                "{source}: no `{name}` column (export the rows again with `bench csv`)"
            ))
        })
    };
    let (pack, mode, model, solved, dollars) = (
        col("pack")?,
        col("mode")?,
        col("model")?,
        col("solved")?,
        col("dollars")?,
    );
    let optional = |name: &str| columns.iter().position(|c| c == name);
    let (calls, tokens, depth, wall_ms) = (
        optional("calls"),
        optional("tokens"),
        optional("depth"),
        optional("wall_ms"),
    );
    let mut rows = vec![];
    for (i, line) in lines.enumerate() {
        let cells = split_csv(line);
        let get = |idx: usize| cells.get(idx).map(String::as_str).unwrap_or("");
        let num = |idx: Option<usize>| idx.map(get).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let model_name = get(model).trim().to_string();
        if model_name.is_empty() {
            return Err(HarnessError::Config(format!(
                "{source} line {}: empty `model`; every row needs the model it ran on",
                i + 2
            )));
        }
        rows.push(EvalRecord {
            pack: get(pack).to_string(),
            mode: get(mode).to_string(),
            model: model_name,
            solved: get(solved) == "true",
            calls: num(calls),
            tokens: num(tokens),
            dollars: get(dollars).parse().unwrap_or(0.0),
            depth: num(depth),
            wall_ms: num(wall_ms),
        });
    }
    Ok(rows)
}

/// Split one CSV line, honouring double quotes (`bench csv` quotes a cell
/// that holds a comma or a quote and doubles the quotes inside).
fn split_csv(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cell)),
            _ => cell.push(c),
        }
    }
    out.push(cell);
    out
}

/// The aggregate of one (pack, mode, model).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    /// Rows.
    pub tasks: usize,
    /// Rows the oracle passed.
    pub solved: usize,
    /// Sum of calls.
    pub calls: f64,
    /// Sum of tokens.
    pub tokens: f64,
    /// Sum of dollars.
    pub dollars: f64,
    /// Sum of depth.
    pub depth: f64,
    /// Sum of wall-clock milliseconds.
    pub wall_ms: f64,
}

impl Cell {
    /// Solved over tasks, 0 for an empty cell.
    pub fn pass_rate(&self) -> f64 {
        if self.tasks == 0 {
            0.0
        } else {
            self.solved as f64 / self.tasks as f64
        }
    }
    fn per_task(&self, total: f64) -> f64 {
        if self.tasks == 0 {
            0.0
        } else {
            total / self.tasks as f64
        }
    }
    /// Mean dollars per task.
    pub fn dollars_per_task(&self) -> f64 {
        self.per_task(self.dollars)
    }
    /// Mean calls per task.
    pub fn calls_per_task(&self) -> f64 {
        self.per_task(self.calls)
    }
    /// Mean tokens per task.
    pub fn tokens_per_task(&self) -> f64 {
        self.per_task(self.tokens)
    }
    /// Mean depth.
    pub fn mean_depth(&self) -> f64 {
        self.per_task(self.depth)
    }
    /// Mean wall-clock seconds per task.
    pub fn seconds_per_task(&self) -> f64 {
        self.per_task(self.wall_ms) / 1000.0
    }
}

/// The modes in the order the tables and plots show them.
pub const MODES: [&str; 3] = ["learning", "frozen", "plain"];

/// Results grouped by pack, mode and model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Results {
    cells: BTreeMap<(String, String, String), Cell>,
    /// Models in order of first appearance in the input.
    models: Vec<String>,
}

impl Results {
    /// Aggregate rows. Models keep the order they first appear in, so the
    /// first CSV given decides the first column.
    pub fn from_rows(rows: &[EvalRecord]) -> Self {
        let mut r = Results::default();
        for row in rows {
            if !r.models.contains(&row.model) {
                r.models.push(row.model.clone());
            }
            let cell = r
                .cells
                .entry((row.pack.clone(), row.mode.clone(), row.model.clone()))
                .or_default();
            cell.tasks += 1;
            cell.solved += usize::from(row.solved);
            cell.calls += row.calls;
            cell.tokens += row.tokens;
            cell.dollars += row.dollars;
            cell.depth += row.depth;
            cell.wall_ms += row.wall_ms;
        }
        r
    }

    /// Models, in column order.
    pub fn models(&self) -> &[String] {
        &self.models
    }

    /// Packs, alphabetically.
    pub fn packs(&self) -> Vec<String> {
        self.cells
            .keys()
            .map(|(p, _, _)| p.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Modes present for a pack, in `MODES` order, then any other label.
    pub fn modes(&self, pack: &str) -> Vec<String> {
        let present: BTreeSet<String> = self
            .cells
            .keys()
            .filter(|(p, _, _)| p == pack)
            .map(|(_, m, _)| m.clone())
            .collect();
        let mut out: Vec<String> = MODES
            .iter()
            .filter(|m| present.contains(**m))
            .map(|m| m.to_string())
            .collect();
        out.extend(present.into_iter().filter(|m| !MODES.contains(&m.as_str())));
        out
    }

    /// The cell for a (pack, mode, model), if that model ran that pack in that mode.
    pub fn cell(&self, pack: &str, mode: &str, model: &str) -> Option<&Cell> {
        self.cells
            .get(&(pack.to_string(), mode.to_string(), model.to_string()))
    }

    /// The summary table: one row per pack, mode and model, with the
    /// metrics as columns. Every model has a row for every pack and mode
    /// any model ran, so a model that has not run one shows a row with
    /// blank metrics rather than disappearing.
    pub fn summary_markdown(&self) -> String {
        let mut out = String::from(
            "| Pack | Mode | Model | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Seconds/task |\n|---|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
        );
        for pack in self.packs() {
            for mode in self.modes(&pack) {
                for m in &self.models {
                    let _ = write!(out, "| `{pack}` | {mode} | {} |", display_name(m));
                    match self.cell(&pack, &mode, m) {
                        Some(c) => {
                            let _ = writeln!(
                                out,
                                " {} | {} | {} | {} | {} | {} | {} |",
                                c.tasks,
                                pass(c),
                                money(c.dollars_per_task()),
                                money(c.dollars),
                                one_decimal(c.calls_per_task()),
                                thousands(c.tokens_per_task()),
                                one_decimal(c.seconds_per_task())
                            );
                        }
                        None => out.push_str(" | | | | | | |\n"),
                    }
                }
            }
        }
        out
    }

    /// The quality-against-cost plot for one pack: dollars per task against
    /// pass rate, one point per model and mode, one colour per model, the
    /// mode written next to each point and the Pareto frontier dashed
    /// through the points nothing beats on both axes.
    pub fn pareto_chart(&self, pack: &str) -> Chart {
        let mut chart = Chart::new(
            &format!("{pack}: pass rate against cost"),
            "dollars per task",
            "pass rate",
        );
        chart.scatter = true;
        chart.frontier = true;
        chart.x_from_zero = true;
        chart.percent_y = true;
        for m in &self.models {
            let mut points = vec![];
            for mode in self.modes(pack) {
                if let Some(c) = self.cell(pack, &mode, m) {
                    let p = (c.dollars_per_task(), c.pass_rate());
                    points.push(p);
                    chart.annotations.push((p.0, p.1, mode.clone()));
                }
            }
            if !points.is_empty() {
                chart.series.push(Series {
                    label: display_name(m),
                    points,
                });
            }
        }
        chart
    }

    /// File name of a pack's Pareto plot.
    pub fn pareto_file(pack: &str) -> String {
        format!("{pack}-pareto.svg")
    }

    /// Every pack's Pareto plot as `(file name, SVG)`.
    pub fn pareto_svgs(&self) -> Vec<(String, String)> {
        self.packs()
            .iter()
            .map(|p| (Self::pareto_file(p), render(&self.pareto_chart(p))))
            .collect()
    }

    /// The whole results section as Markdown: the summary table, then one
    /// collapsible block per plotted pack (every pack when `plotted` is
    /// empty) holding its plot. `image_dir` is where the SVGs are,
    /// relative to the document the Markdown goes in. Packs where every
    /// mode scores the same (the four original packs on Opus 5.5) have
    /// nothing to show on a Pareto plot, so the README plots the hard
    /// packs only.
    pub fn markdown(&self, image_dir: &str, plotted: &[String]) -> String {
        let mut out = String::new();
        out.push_str(&self.summary_markdown());
        out.push('\n');
        let _ = writeln!(
            out,
            "Models: {}.\n",
            self.models
                .iter()
                .map(|m| format!("{} is `{m}`", display_name(m)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let dir = image_dir.trim_end_matches('/');
        for pack in self.packs() {
            if !(plotted.is_empty() || plotted.contains(&pack)) {
                continue;
            }
            let image = if dir.is_empty() {
                Self::pareto_file(&pack)
            } else {
                format!("{dir}/{}", Self::pareto_file(&pack))
            };
            let _ = write!(
                out,
                "<details>\n<summary><code>{pack}</code>: pass rate against cost per task, every model and mode</summary>\n\n![{pack}: pass rate against cost]({image})\n\n</details>\n\n"
            );
        }
        out
    }
}

/// Opening marker of the generated block in a Markdown document.
pub const BEGIN_MARKER: &str = "<!-- bench-results:begin -->";
/// Closing marker of the generated block in a Markdown document.
pub const END_MARKER: &str = "<!-- bench-results:end -->";

/// Replace what lies between the markers in `document` with `block`,
/// keeping the markers. An error if either marker is missing.
pub fn splice(document: &str, block: &str) -> Result<String, HarnessError> {
    let start = document
        .find(BEGIN_MARKER)
        .ok_or_else(|| HarnessError::Config(format!("document has no `{BEGIN_MARKER}` marker")))?;
    let after_start = start + BEGIN_MARKER.len();
    let end = document[after_start..]
        .find(END_MARKER)
        .map(|i| i + after_start)
        .ok_or_else(|| {
            HarnessError::Config(format!(
                "document has no `{END_MARKER}` marker after the opening one"
            ))
        })?;
    Ok(format!(
        "{}{BEGIN_MARKER}\n{}\n{END_MARKER}{}",
        &document[..start],
        block.trim_end_matches('\n'),
        &document[end + END_MARKER.len()..]
    ))
}

/// The path from the directory holding `document` to `dir`, for image
/// links: both are taken relative to the current directory, and a `dir`
/// that is not under the document's directory is given with `..` steps.
pub fn relative_dir(document: &Path, dir: &Path) -> String {
    let base: Vec<_> = document
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    let target: Vec<_> = dir
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); base.len() - common];
    parts.extend(
        target[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// A readable model name from its identifier: `claude-opus-5-5` reads
/// `Claude Opus 5.5`, `claude-haiku-4-5-20251001` reads `Claude Haiku 4.5`
/// (the date is dropped), `gpt-5.6` reads `GPT 5.6`. An id with no
/// hyphens is shown as is.
pub fn display_name(model: &str) -> String {
    let parts: Vec<&str> = model
        .split('-')
        .filter(|p| !(p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())))
        .collect();
    let mut words: Vec<String> = vec![];
    for part in parts {
        let numeric = part.chars().all(|c| c.is_ascii_digit() || c == '.') && !part.is_empty();
        match (numeric, words.last_mut()) {
            (true, Some(last)) if last.chars().all(|c| c.is_ascii_digit() || c == '.') => {
                last.push('.');
                last.push_str(part);
            }
            (true, _) => words.push(part.to_string()),
            (false, _) => {
                let word = if part.len() <= 3 {
                    part.to_uppercase()
                } else {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                };
                words.push(word);
            }
        }
    }
    if words.is_empty() {
        model.to_string()
    } else {
        words.join(" ")
    }
}

fn pass(c: &Cell) -> String {
    format!("{}/{} ({:.0}%)", c.solved, c.tasks, c.pass_rate() * 100.0)
}

fn money(v: f64) -> String {
    if v >= 1.0 {
        format!("${v:.2}")
    } else {
        format!("${v:.3}")
    }
}

fn one_decimal(v: f64) -> String {
    format!("{v:.1}")
}

fn thousands(v: f64) -> String {
    let whole = format!("{:.0}", v.round());
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str =
        "run,pack,mode,model,seq,task,kind,solved,calls,tokens,dollars,depth,turns,wall_ms\n\
r1,memo,frozen,claude-haiku-4-5-20251001,0,a,memo,true,10,1000,0.10,0,3,2000\n\
r1,memo,frozen,claude-haiku-4-5-20251001,1,b,memo,false,30,3000,0.30,1,5,4000\n\
r2,memo,plain,claude-haiku-4-5-20251001,0,a,memo,true,2,500,0.05,0,2,1000\n\
r3,memo,learning,claude-opus-5-5,0,a,memo,true,4,800,0.40,0,2,3000\n\
r4,terminal,plain,claude-opus-5-5,0,\"x,y\",terminal,true,1,100,0.01,0,1,500\n";

    #[test]
    fn parses_quoted_cells_and_requires_a_model() {
        let rows = parse_csv(CSV, "t").unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[4].pack, "terminal");
        assert_eq!(rows[1].wall_ms, 4000.0);
        let err = parse_csv("run,pack,mode,solved,dollars\nr,p,m,true,1\n", "old.csv")
            .unwrap_err()
            .to_string();
        assert!(err.contains("old.csv: no `model` column"), "{err}");
        let err = parse_csv("pack,mode,model,solved,dollars\np,m,,true,1\n", "e.csv")
            .unwrap_err()
            .to_string();
        assert!(err.contains("line 2: empty `model`"), "{err}");
    }

    #[test]
    fn aggregates_and_leaves_missing_cells_blank() {
        let r = Results::from_rows(&parse_csv(CSV, "t").unwrap());
        assert_eq!(r.models(), ["claude-haiku-4-5-20251001", "claude-opus-5-5"]);
        assert_eq!(r.packs(), ["memo", "terminal"]);
        assert_eq!(r.modes("memo"), ["learning", "frozen", "plain"]);
        let haiku = r
            .cell("memo", "frozen", "claude-haiku-4-5-20251001")
            .unwrap();
        assert_eq!(haiku.tasks, 2);
        assert_eq!(haiku.pass_rate(), 0.5);
        assert!((haiku.dollars_per_task() - 0.2).abs() < 1e-9);
        assert_eq!(haiku.seconds_per_task(), 3.0);
        let md = r.summary_markdown();
        let lines: Vec<&str> = md.lines().collect();
        assert_eq!(
            lines[0],
            "| Pack | Mode | Model | Tasks | Pass | $/task | Total $ | Calls/task | Tokens/task | Seconds/task |"
        );
        // Haiku never ran memo learning: a row with blank metrics, not zeros.
        assert_eq!(
            lines[2],
            "| `memo` | learning | Claude Haiku 4.5 | | | | | | | |"
        );
        assert_eq!(
            lines[3],
            "| `memo` | learning | Claude Opus 5.5 | 1 | 1/1 (100%) | $0.400 | $0.400 | 4.0 | 800 | 3.0 |"
        );
        assert_eq!(
            lines[4],
            "| `memo` | frozen | Claude Haiku 4.5 | 2 | 1/2 (50%) | $0.200 | $0.400 | 20.0 | 2,000 | 3.0 |"
        );
        assert_eq!(
            lines[5],
            "| `memo` | frozen | Claude Opus 5.5 | | | | | | | |"
        );
        // Two packs, memo in three modes and terminal in one, two models.
        assert_eq!(lines.len(), 2 + (3 + 1) * 2);
    }

    #[test]
    fn pareto_chart_has_a_series_per_model_and_a_label_per_point() {
        let r = Results::from_rows(&parse_csv(CSV, "t").unwrap());
        let chart = r.pareto_chart("memo");
        assert_eq!(chart.series.len(), 2);
        assert_eq!(chart.series[0].label, "Claude Haiku 4.5");
        assert_eq!(chart.series[0].points.len(), 2);
        assert_eq!(chart.annotations.len(), 3);
        assert!(chart.frontier && chart.scatter && chart.x_from_zero && chart.percent_y);
        let svgs = r.pareto_svgs();
        assert_eq!(svgs[0].0, "memo-pareto.svg");
        assert!(svgs[0].1.contains("frozen"));
        let md = r.markdown("plots/results", &[]);
        assert!(md.contains("![memo: pass rate against cost](plots/results/memo-pareto.svg)"));
        assert!(md.contains("![terminal: pass rate against cost]"));
        assert!(md.contains("<details>"));
        assert!(md.contains("Claude Opus 5.5 is `claude-opus-5-5`"));
        // Only the packs asked for get a plot; the others have no block.
        let md = r.markdown("plots/results", &["memo".to_string()]);
        assert!(md.contains("![memo: pass rate against cost]"));
        assert!(!md.contains("terminal</code>"), "{md}");
    }

    #[test]
    fn splices_between_markers_and_computes_relative_dirs() {
        let doc = "intro\n<!-- bench-results:begin -->\nold\n<!-- bench-results:end -->\noutro\n";
        let out = splice(doc, "new\n").unwrap();
        assert_eq!(
            out,
            "intro\n<!-- bench-results:begin -->\nnew\n<!-- bench-results:end -->\noutro\n"
        );
        // Idempotent: splicing the same block again changes nothing.
        assert_eq!(splice(&out, "new\n").unwrap(), out);
        assert!(splice("no markers", "x").is_err());
        assert_eq!(
            relative_dir(Path::new("README.md"), Path::new("plots/results")),
            "plots/results"
        );
        assert_eq!(
            relative_dir(Path::new("docs/WRITEUP.md"), Path::new("plots/results")),
            "../plots/results"
        );
        assert_eq!(
            relative_dir(Path::new("./README.md"), Path::new("./plots")),
            "plots"
        );
    }

    #[test]
    fn display_names_read_well() {
        assert_eq!(display_name("claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(
            display_name("claude-haiku-4-5-20251001"),
            "Claude Haiku 4.5"
        );
        assert_eq!(display_name("claude-sonnet-5-5"), "Claude Sonnet 5.5");
        assert_eq!(display_name("gpt-5.6"), "GPT 5.6");
        assert_eq!(display_name("llama"), "Llama");
        assert_eq!(thousands(1234567.0), "1,234,567");
        assert_eq!(thousands(999.0), "999");
    }
}
