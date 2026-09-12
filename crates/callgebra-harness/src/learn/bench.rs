//! Benchmark runs over task packs: the same stream under three modes,
//! recorded per task in `evals` so accuracy, cost and the learning curve are
//! queries. Modes: `learning` (playbook on, adoption on), `frozen` (no
//! playbook, nothing adopted: the control), `plain` (the tool-calling agent
//! baseline on the same provider and budgets).

use super::packs::{prepare_workspace, Pack};
use super::plain::{self, PlainConfig};
use super::verify::Verdict;
use super::{Learn, Task};
use crate::{HarnessError, PlaybookExample};
use callgebra_core::{Batch, Value};
use callgebra_store::duckdb::literal;
use serde::{Deserialize, Serialize};
use std::path::Path;

const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS evals (run VARCHAR, pack VARCHAR, mode VARCHAR, seq INTEGER, task VARCHAR, kind VARCHAR, solved BOOLEAN, detail VARCHAR, calls BIGINT, tokens BIGINT, dollars DOUBLE, depth INTEGER, turns INTEGER, wall_ms BIGINT, recorded_at TIMESTAMP)",
    "CREATE TABLE IF NOT EXISTS bench_runs (run VARCHAR PRIMARY KEY, pack VARCHAR, mode VARCHAR, tasks INTEGER, solved INTEGER, dollars DOUBLE, started_at TIMESTAMP, finished_at TIMESTAMP, note VARCHAR)",
];

/// How a pack is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Callgebra with the playbook shown and adoption on.
    Learning,
    /// Callgebra with a frozen catalog: no playbook, nothing adopted.
    Frozen,
    /// The plain tool-calling agent on the same provider and budgets.
    Plain,
}

impl Mode {
    /// Name used in tables.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Learning => "learning",
            Mode::Frozen => "frozen",
            Mode::Plain => "plain",
        }
    }

    /// Parse a label.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "learning" | "learn" | "on" => Some(Mode::Learning),
            "frozen" | "off" | "control" => Some(Mode::Frozen),
            "plain" | "baseline" | "agent" => Some(Mode::Plain),
            _ => None,
        }
    }
}

/// One task's result in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalRow {
    /// Position in the stream.
    pub seq: usize,
    /// Task id within the pack.
    pub task: String,
    /// Solved.
    pub solved: bool,
    /// Oracle detail.
    pub detail: String,
    /// Model calls.
    pub calls: u64,
    /// Tokens.
    pub tokens: u64,
    /// Dollars.
    pub dollars: f64,
    /// Turns.
    pub turns: u32,
}

/// A whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    /// Run id.
    pub run: String,
    /// Pack.
    pub pack: String,
    /// Mode.
    pub mode: Mode,
    /// Per task.
    pub rows: Vec<EvalRow>,
}

impl BenchReport {
    /// Solved fraction.
    pub fn accuracy(&self) -> f64 {
        if self.rows.is_empty() {
            0.0
        } else {
            self.rows.iter().filter(|r| r.solved).count() as f64 / self.rows.len() as f64
        }
    }
    /// Total dollars.
    pub fn dollars(&self) -> f64 {
        self.rows.iter().map(|r| r.dollars).sum()
    }
    /// Total calls.
    pub fn calls(&self) -> u64 {
        self.rows.iter().map(|r| r.calls).sum()
    }
    /// Rolling solve rate over a window, one value per task: the learning curve.
    pub fn curve(&self, window: usize) -> Vec<f64> {
        let w = window.max(1);
        (0..self.rows.len())
            .map(|i| {
                let lo = i.saturating_sub(w - 1);
                let slice = &self.rows[lo..=i];
                slice.iter().filter(|r| r.solved).count() as f64 / slice.len() as f64
            })
            .collect()
    }
}

fn s(v: &str) -> String {
    literal(&Value::Text(v.into()))
}

fn text_at(b: &Batch, row: usize, col: usize) -> String {
    b.rows
        .get(row)
        .and_then(|r| r.get(col))
        .map(|v| match v {
            Value::Null => String::new(),
            other => other.render(),
        })
        .unwrap_or_default()
}

impl Learn {
    /// Create the bench tables.
    pub async fn init_bench(&self) -> Result<(), HarnessError> {
        for ddl in DDL {
            self.store().execute(ddl).await?;
        }
        Ok(())
    }

    /// Run a pack in one mode. Every task gets a fresh workspace (seeded
    /// from `workspace_from` and its setup commands). Results land in
    /// `evals` and `bench_runs`.
    pub async fn bench(
        &self,
        pack_dir: &Path,
        pack: &Pack,
        mode: Mode,
        limit: Option<usize>,
    ) -> Result<BenchReport, HarnessError> {
        self.init_bench().await?;
        let run = callgebra_core::RunId::new().to_string();
        self.store()
            .execute(&format!(
                "INSERT INTO bench_runs VALUES ({}, {}, {}, 0, 0, 0.0, now(), NULL, NULL)",
                s(&run),
                s(&pack.name),
                s(mode.label())
            ))
            .await?;
        let mut rows = vec![];
        let n = limit.unwrap_or(pack.tasks.len()).min(pack.tasks.len());
        for (seq, pt) in pack.tasks.iter().take(n).enumerate() {
            let generated = pack.to_generated(pack_dir, pt)?;
            // Every pack task runs in its own fresh workspace, so tasks and
            // concurrent runs cannot see each other's files.
            let ws = prepare_workspace(pack_dir, pt).await?;
            let workspace = ws.path().to_path_buf();
            let id = self
                .add_generated(&generated, &format!("pack:{}", pack.name))
                .await?;
            let task = self
                .task(&id)
                .await?
                .ok_or_else(|| HarnessError::Corrupt("task vanished".into()))?;
            let (verdict, calls, tokens, dollars, turns, depth, wall_ms) = match mode {
                Mode::Plain => self.bench_plain(&task, &workspace).await,
                Mode::Learning | Mode::Frozen => {
                    self.bench_callgebra(&task, &workspace, mode == Mode::Learning)
                        .await?
                }
            };
            self.store()
                .execute(&format!(
                    "UPDATE tasks SET status = {}, attempts = attempts + 1, last_detail = {}, updated_at = now() WHERE id = {}",
                    s(if verdict.pass { "solved" } else { "failed" }),
                    s(&verdict.detail),
                    s(&id)
                ))
                .await?;
            self.store()
                .execute(&format!(
                    "INSERT INTO evals VALUES ({}, {}, {}, {seq}, {}, {}, {}, {}, {calls}, {tokens}, {dollars}, {depth}, {turns}, {wall_ms}, now())",
                    s(&run),
                    s(&pack.name),
                    s(mode.label()),
                    s(&pt.id),
                    s(&task.kind),
                    verdict.pass,
                    s(&verdict.detail)
                ))
                .await?;
            rows.push(EvalRow {
                seq,
                task: pt.id.clone(),
                solved: verdict.pass,
                detail: verdict.detail,
                calls,
                tokens,
                dollars,
                turns,
            });
        }
        let report = BenchReport {
            run: run.clone(),
            pack: pack.name.clone(),
            mode,
            rows,
        };
        self.store()
            .execute(&format!(
                "UPDATE bench_runs SET tasks = {}, solved = {}, dollars = {}, finished_at = now() WHERE run = {}",
                report.rows.len(),
                report.rows.iter().filter(|r| r.solved).count(),
                report.dollars(),
                s(&run)
            ))
            .await?;
        Ok(report)
    }

    async fn bench_callgebra(
        &self,
        task: &Task,
        workspace: &Path,
        learning: bool,
    ) -> Result<(Verdict, u64, u64, f64, u32, i64, u64), HarnessError> {
        let playbook: Vec<PlaybookExample> = if learning {
            self.playbook_for(&task.kind).await?
        } else {
            vec![]
        };
        let report = self
            .run_task_in(task, playbook, false, Some(workspace))
            .await?;
        let verdict = self.judge_in(task, &report, workspace).await;
        if learning {
            let usage = report.root.usage;
            self.rate(task, verdict.pass, usage.dollars).await?;
            if verdict.pass {
                if let Some(sql) = super::winning_sql(&report) {
                    let v = self.record_candidate(task, &sql, &report).await?;
                    self.gate(&task.kind, v).await?;
                }
            }
        }
        let u = &report.root.usage;
        let depth = self.run_depth(&report.run.to_string()).await;
        Ok((
            verdict,
            u.calls,
            u.tokens,
            u.dollars,
            report.root.turns,
            depth,
            u.wall.as_millis() as u64,
        ))
    }

    async fn bench_plain(
        &self,
        task: &Task,
        workspace: &Path,
    ) -> (Verdict, u64, u64, f64, u32, i64, u64) {
        let Some(provider) = self.harness_cfg().provider.clone() else {
            return (
                Verdict {
                    pass: false,
                    detail: "no provider".into(),
                },
                0,
                0,
                0.0,
                0,
                0,
                0,
            );
        };
        let cfg = PlainConfig {
            provider: provider.clone(),
            alias: "root".into(),
            workspace: workspace.to_path_buf(),
            max_turns: self.harness_cfg().max_turns,
            budget: self.harness_cfg().budget.clone(),
            max_tokens: self.harness_cfg().max_tokens,
        };
        let r = plain::run(&cfg, &task.task, task.context.as_deref()).await;
        let verdict = match &r.answer {
            Some(answer) => {
                let judge = Some((provider, self.judge_alias().to_string()));
                task.verify.check(answer, workspace, judge).await
            }
            None => Verdict {
                pass: false,
                detail: format!("no answer: {}", r.outcome),
            },
        };
        (
            verdict,
            r.usage.calls,
            r.usage.tokens,
            r.usage.dollars,
            r.turns,
            0,
            r.usage.wall.as_millis() as u64,
        )
    }

    /// Summary per (pack, mode) over every recorded run: tasks, accuracy,
    /// calls and dollars per task.
    pub async fn bench_summary(&self) -> Result<Batch, HarnessError> {
        self.init_bench().await?;
        Ok(self
            .store()
            .query(
                "SELECT pack, mode, COUNT(*) AS tasks, ROUND(AVG(CASE WHEN solved THEN 1.0 ELSE 0.0 END), 3) AS accuracy, \
                 ROUND(AVG(calls), 2) AS calls_per_task, ROUND(SUM(dollars), 4) AS dollars, ROUND(AVG(tokens)) AS tokens_per_task, \
                 ROUND(AVG(depth), 2) AS depth FROM evals GROUP BY pack, mode ORDER BY pack, mode",
            )
            .await?)
    }

    /// The learning curve of one run as `(seq, solved)` pairs.
    pub async fn bench_curve(&self, run: &str) -> Result<Vec<(usize, bool)>, HarnessError> {
        let b = self
            .store()
            .query(&format!(
                "SELECT seq, solved FROM evals WHERE run = {} ORDER BY seq",
                s(run)
            ))
            .await?;
        Ok((0..b.rows.len())
            .map(|i| {
                (
                    text_at(&b, i, 0).parse().unwrap_or(0),
                    text_at(&b, i, 1) == "true",
                )
            })
            .collect())
    }

    /// Every eval row as CSV, for plotting elsewhere.
    pub async fn bench_csv(&self) -> Result<String, HarnessError> {
        self.init_bench().await?;
        let b = self
            .store()
            .query("SELECT run, pack, mode, seq, task, kind, solved, calls, tokens, dollars, depth, turns, wall_ms FROM evals ORDER BY run, seq")
            .await?;
        let mut out = String::from(
            "run,pack,mode,seq,task,kind,solved,calls,tokens,dollars,depth,turns,wall_ms\n",
        );
        for row in &b.rows {
            let cells: Vec<String> = row
                .iter()
                .map(|v| {
                    let t = match v {
                        Value::Null => String::new(),
                        other => other.render(),
                    };
                    if t.contains(',') || t.contains('"') {
                        format!("\"{}\"", t.replace('"', "\"\""))
                    } else {
                        t
                    }
                })
                .collect();
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        Ok(out)
    }
}

/// Render a curve as a sparkline of block characters.
pub fn sparkline(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values
        .iter()
        .map(|v| BARS[((v.clamp(0.0, 1.0) * 7.0).round()) as usize])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_and_sparklines() {
        let r = BenchReport {
            run: "r".into(),
            pack: "p".into(),
            mode: Mode::Learning,
            rows: [false, true, true, false, true]
                .iter()
                .enumerate()
                .map(|(i, s)| EvalRow {
                    seq: i,
                    task: i.to_string(),
                    solved: *s,
                    detail: String::new(),
                    calls: 1,
                    tokens: 10,
                    dollars: 0.01,
                    turns: 1,
                })
                .collect(),
        };
        assert!((r.accuracy() - 0.6).abs() < 1e-12);
        assert_eq!(r.curve(2), vec![0.0, 0.5, 1.0, 0.5, 0.5]);
        assert_eq!(sparkline(&[0.0, 0.5, 1.0]), "▁▅█");
        assert_eq!(Mode::parse("off"), Some(Mode::Frozen));
        assert_eq!(Mode::parse("nope"), None);
    }
}
