//! The continual, self-learning loop: a `tasks` table kept non-empty by
//! generators, a curriculum that picks the next task near even odds, a
//! solver run through the harness, an oracle that judges `FINAL`, ratings
//! that move on the outcome, and a playbook of winning SQL that is adopted
//! only after it wins a replay eval and can be reverted.
//!
//! Everything the loop knows lives in tables in the store (`tasks`,
//! `task_ratings`, `solver_ratings`, `generator_state`, `playbook`,
//! `playbook_evals`, and the `trace_tasks` view), so a restart is a resume and the
//! morning report is a query.

pub mod bench;
pub mod generators;
pub mod packs;
pub mod plain;
pub mod ratings;
pub mod verify;

use crate::session::{Harness, HarnessConfig, RunReport};
use crate::{HarnessError, Outcome, PlaybookExample};
use callgebra_core::{Batch, Value};
use callgebra_store::duckdb::literal;
use callgebra_store::DuckDbStore;
use generators::{GeneratedTask, GENERATORS};
use serde::{Deserialize, Serialize};
use verify::{Verdict, Verify};

const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS tasks (id VARCHAR PRIMARY KEY, kind VARCHAR, generator VARCHAR, source VARCHAR, dial DOUBLE, difficulty DOUBLE, task VARCHAR, context VARCHAR, verify VARCHAR, status VARCHAR, attempts INTEGER, last_run VARCHAR, last_detail VARCHAR, created_at TIMESTAMP, updated_at TIMESTAMP)",
    "CREATE TABLE IF NOT EXISTS task_ratings (task VARCHAR PRIMARY KEY, rating DOUBLE, games INTEGER)",
    "CREATE TABLE IF NOT EXISTS solver_ratings (solver VARCHAR PRIMARY KEY, rating DOUBLE, games INTEGER, solved INTEGER, dollars DOUBLE)",
    "CREATE TABLE IF NOT EXISTS generator_state (generator VARCHAR PRIMARY KEY, dial DOUBLE, solve_rate DOUBLE, tried INTEGER, solved INTEGER, updated_at TIMESTAMP)",
    "CREATE TABLE IF NOT EXISTS playbook (version INTEGER PRIMARY KEY, kind VARCHAR, sql VARCHAR, wins INTEGER, tries INTEGER, avg_cost DOUBLE, adopted BOOLEAN, learned_from VARCHAR, eval_note VARCHAR, created_at TIMESTAMP)",
    "CREATE TABLE IF NOT EXISTS playbook_evals (run_at TIMESTAMP, kind VARCHAR, candidate INTEGER, baseline_solved INTEGER, candidate_solved INTEGER, total INTEGER, baseline_dollars DOUBLE, candidate_dollars DOUBLE, adopted BOOLEAN)",
    "CREATE TABLE IF NOT EXISTS attempts (task VARCHAR, run VARCHAR, solver VARCHAR, solved BOOLEAN, detail VARCHAR, calls BIGINT, dollars DOUBLE, depth INTEGER, playbook_version INTEGER, recorded_at TIMESTAMP)",
    "CREATE OR REPLACE VIEW trace_tasks AS SELECT t.id AS task, t.kind, t.generator, t.source, t.dial, t.difficulty, t.status, a.run, a.solver, CASE WHEN a.solved THEN 1.0 ELSE 0.0 END AS solved, a.calls, a.dollars, a.depth, a.recorded_at FROM tasks t JOIN attempts a ON a.task = t.id",
];

/// A stored task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Id.
    pub id: String,
    /// Kind, for playbook lookup.
    pub kind: String,
    /// Generator, or `user` / `proposer`.
    pub generator: String,
    /// Hardness dial at generation.
    pub dial: f64,
    /// Rating prior.
    pub difficulty: f64,
    /// Task text.
    pub task: String,
    /// Context, if any.
    pub context: Option<String>,
    /// Oracle.
    pub verify: Verify,
    /// `pending`, `running`, `solved`, `failed`, `needs_review`.
    pub status: String,
    /// Attempts so far.
    pub attempts: u32,
}

/// The result of one loop step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    /// The task attempted.
    pub task: String,
    /// Its kind.
    pub kind: String,
    /// Generator.
    pub generator: String,
    /// Solved.
    pub solved: bool,
    /// Oracle detail.
    pub detail: String,
    /// The run.
    pub run: String,
    /// Model calls spent.
    pub calls: u64,
    /// Dollars spent.
    pub dollars: f64,
    /// Whether a playbook candidate was recorded from this solve.
    pub playbook_candidate: Option<i64>,
    /// Whether that candidate was adopted after the replay eval.
    pub adopted: Option<bool>,
}

/// Limits for an unattended run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunLimits {
    /// Stop after this many tasks.
    pub max_tasks: usize,
    /// Stop when this much has been spent (dollars).
    pub max_dollars: Option<f64>,
    /// Stop after this long.
    pub max_wall: Option<std::time::Duration>,
    /// Generators to keep fed (empty: all).
    pub generators: Vec<String>,
    /// Tasks to keep pending per generator.
    pub queue_depth: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_tasks: 10,
            max_dollars: None,
            max_wall: None,
            generators: vec![],
            queue_depth: 3,
        }
    }
}

/// Learning knobs.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnConfig {
    /// Alias the judge oracle uses.
    pub judge_alias: String,
    /// Tasks of a kind replayed to gate a playbook candidate (0 disables
    /// the gate: candidates are adopted outright).
    pub replay_sample: usize,
    /// Dial step per curriculum adjustment.
    pub dial_step: f64,
    /// Playbook entries shown per task kind.
    pub playbook_examples: usize,
}

impl Default for LearnConfig {
    fn default() -> Self {
        Self {
            judge_alias: "judge".into(),
            replay_sample: 3,
            dial_step: 0.1,
            playbook_examples: 2,
        }
    }
}

/// The continual loop over one store.
pub struct Learn {
    store: DuckDbStore,
    harness_cfg: HarnessConfig,
    cfg: LearnConfig,
    solver: String,
}

fn now() -> String {
    "now()".to_string()
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

fn float_at(b: &Batch, row: usize, col: usize) -> f64 {
    text_at(b, row, col).parse().unwrap_or(0.0)
}

fn int_at(b: &Batch, row: usize, col: usize) -> i64 {
    text_at(b, row, col).parse().unwrap_or(0)
}

impl Learn {
    /// Open the loop over a store; creates its tables.
    pub async fn new(
        store: DuckDbStore,
        harness_cfg: HarnessConfig,
        cfg: LearnConfig,
    ) -> Result<Self, HarnessError> {
        for ddl in DDL {
            store.execute(ddl).await?;
        }
        let solver = format!(
            "{}/turns{}/depth{}",
            harness_cfg
                .provider
                .as_ref()
                .map(|p| p.name().to_string())
                .unwrap_or_else(|| "none".into()),
            harness_cfg.max_turns,
            harness_cfg.max_depth
        );
        Ok(Self {
            store,
            harness_cfg,
            cfg,
            solver,
        })
    }

    /// The solver configuration label ratings are kept under.
    pub fn solver(&self) -> &str {
        &self.solver
    }

    /// The store.
    pub fn store(&self) -> &DuckDbStore {
        &self.store
    }

    /// The harness configuration runs are derived from.
    pub fn harness_cfg(&self) -> &HarnessConfig {
        &self.harness_cfg
    }

    /// Alias the judge oracle uses.
    pub fn judge_alias(&self) -> &str {
        &self.cfg.judge_alias
    }

    /// Add a user task. `verify` defaults to human review.
    pub async fn add_task(
        &self,
        kind: &str,
        task: &str,
        context: Option<String>,
        verify: Option<Verify>,
    ) -> Result<String, HarnessError> {
        let t = GeneratedTask {
            generator: "user".into(),
            kind: kind.to_string(),
            task: task.to_string(),
            context,
            verify: verify.unwrap_or(Verify::Human),
            dial: 0.5,
            difficulty: ratings::BASELINE,
        };
        self.add_generated(&t, "user").await
    }

    /// Generate `count` tasks from a generator at its current dial.
    pub async fn generate(
        &self,
        generator: &str,
        count: usize,
    ) -> Result<Vec<String>, HarnessError> {
        if !GENERATORS.contains(&generator) {
            return Err(HarnessError::Config(format!(
                "unknown generator {generator}; known: {}",
                GENERATORS.join(", ")
            )));
        }
        let dial = self.dial(generator).await?;
        let mut ids = vec![];
        for _ in 0..count {
            let seed = uuid_seed();
            let Some(t) = generators::generate(generator, dial, seed, &self.harness_cfg.workspace)
            else {
                continue;
            };
            ids.push(self.add_generated(&t, "generator").await?);
        }
        Ok(ids)
    }

    /// Ask the model to propose a task with a rubric and a reference answer
    /// (propose/solve/verify self-play); the judge oracle grades solutions,
    /// never the proposer.
    pub async fn propose(&self, topic: &str) -> Result<String, HarnessError> {
        let Some(provider) = &self.harness_cfg.provider else {
            return Err(HarnessError::Config("no provider for the proposer".into()));
        };
        let prompt = format!(
            "Propose one self-contained task about {topic} that a SQL-and-LLM agent can answer with a small relation. \
Aim for a task the agent solves about half the time. Reply with JSON only: {{\"task\": \"...\", \"rubric\": \"what a correct answer must contain\", \"reference\": \"the correct answer\"}}."
        );
        let req = callgebra_llm::CompletionRequest {
            alias: callgebra_core::ModelAlias::root(),
            model: "root".into(),
            system: "You write evaluation tasks with unambiguous answers.".into(),
            messages: vec![callgebra_llm::Message::user(prompt)],
            tools: vec![],
            output_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"},
                    "rubric": {"type": "string"},
                    "reference": {"type": "string"}
                },
                "required": ["task", "rubric", "reference"]
            })),
            max_tokens: 800,
            options: Default::default(),
        };
        let resp = provider
            .complete(req)
            .await
            .map_err(|e| HarnessError::Config(format!("proposer call failed: {e}")))?;
        let text = resp.text();
        let json: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
            HarnessError::Config(format!("proposer did not return JSON: {e}: {text}"))
        })?;
        let field = |k: &str| {
            json.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let t = GeneratedTask {
            generator: "proposer".into(),
            kind: format!("proposed_{}", slug(topic)),
            task: field("task"),
            context: None,
            verify: Verify::Judge {
                rubric: field("rubric"),
                reference: Some(field("reference")).filter(|r| !r.is_empty()),
            },
            dial: 0.5,
            difficulty: ratings::BASELINE,
        };
        if t.task.is_empty() {
            return Err(HarnessError::Config(
                "proposer returned an empty task".into(),
            ));
        }
        self.add_generated(&t, "proposer").await
    }

    /// Store a task (from a generator, a pack or a user) as pending; returns its id.
    pub async fn add_generated(
        &self,
        t: &GeneratedTask,
        source: &str,
    ) -> Result<String, HarnessError> {
        let id = callgebra_core::RunId::new().to_string();
        let verify = serde_json::to_string(&t.verify).unwrap_or_default();
        self.store
            .execute(&format!(
                "INSERT INTO tasks VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, 'pending', 0, NULL, NULL, {}, {})",
                s(&id),
                s(&t.kind),
                s(&t.generator),
                s(source),
                t.dial,
                t.difficulty,
                s(&t.task),
                t.context
                    .as_deref()
                    .map(s)
                    .unwrap_or_else(|| "NULL".into()),
                s(&verify),
                now(),
                now()
            ))
            .await?;
        self.store
            .execute(&format!(
                "INSERT INTO task_ratings VALUES ({}, {}, 0)",
                s(&id),
                t.difficulty
            ))
            .await?;
        Ok(id)
    }

    /// Current dial of a generator (0.3 until it has history).
    pub async fn dial(&self, generator: &str) -> Result<f64, HarnessError> {
        let b = self
            .store
            .query(&format!(
                "SELECT dial FROM generator_state WHERE generator = {}",
                s(generator)
            ))
            .await?;
        Ok(if b.rows.is_empty() {
            0.3
        } else {
            float_at(&b, 0, 0)
        })
    }

    /// Load a task by id.
    pub async fn task(&self, id: &str) -> Result<Option<Task>, HarnessError> {
        let b = self
            .store
            .query(&format!(
                "SELECT id, kind, generator, dial, difficulty, task, context, verify, status, attempts FROM tasks WHERE id = {}",
                s(id)
            ))
            .await?;
        if b.rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(Task {
            id: text_at(&b, 0, 0),
            kind: text_at(&b, 0, 1),
            generator: text_at(&b, 0, 2),
            dial: float_at(&b, 0, 3),
            difficulty: float_at(&b, 0, 4),
            task: text_at(&b, 0, 5),
            context: match b.rows[0].get(6) {
                Some(Value::Null) | None => None,
                Some(v) => Some(v.render()),
            },
            verify: serde_json::from_str(&text_at(&b, 0, 7)).unwrap_or(Verify::Human),
            status: text_at(&b, 0, 8),
            attempts: int_at(&b, 0, 9) as u32,
        }))
    }

    /// The solver's rating and game count.
    pub async fn solver_rating(&self) -> Result<(f64, u64), HarnessError> {
        let b = self
            .store
            .query(&format!(
                "SELECT rating, games FROM solver_ratings WHERE solver = {}",
                s(&self.solver)
            ))
            .await?;
        Ok(if b.rows.is_empty() {
            (ratings::BASELINE, 0)
        } else {
            (float_at(&b, 0, 0), int_at(&b, 0, 1) as u64)
        })
    }

    /// Curriculum: the pending task whose solve probability for this solver
    /// is nearest 0.5, restricted to `generators` when non-empty.
    pub async fn next_task(&self, generators: &[String]) -> Result<Option<Task>, HarnessError> {
        let (solver, _) = self.solver_rating().await?;
        let filter = if generators.is_empty() {
            String::new()
        } else {
            format!(
                " AND t.generator IN ({})",
                generators
                    .iter()
                    .map(|g| s(g))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let b = self
            .store
            .query(&format!(
                "SELECT t.id, r.rating FROM tasks t JOIN task_ratings r ON r.task = t.id WHERE t.status = 'pending'{filter} ORDER BY t.created_at"
            ))
            .await?;
        let candidates: Vec<(String, f64)> = (0..b.rows.len())
            .map(|i| (text_at(&b, i, 0), float_at(&b, i, 1)))
            .collect();
        match ratings::pick_near_half(solver, &candidates) {
            Some(id) => self.task(&id).await,
            None => Ok(None),
        }
    }

    /// Adopted playbook entries for a kind, best first.
    pub async fn playbook_for(&self, kind: &str) -> Result<Vec<PlaybookExample>, HarnessError> {
        let b = self
            .store
            .query(&format!(
                "SELECT kind, sql, wins, tries, version FROM playbook WHERE adopted AND kind = {} ORDER BY wins DESC, avg_cost ASC, version DESC LIMIT {}",
                s(kind),
                self.cfg.playbook_examples
            ))
            .await?;
        Ok((0..b.rows.len())
            .map(|i| PlaybookExample {
                kind: text_at(&b, i, 0),
                sql: text_at(&b, i, 1),
                wins: int_at(&b, i, 2) as u64,
                tries: int_at(&b, i, 3) as u64,
                version: int_at(&b, i, 4),
            })
            .collect())
    }

    async fn run_task(
        &self,
        task: &Task,
        playbook: Vec<PlaybookExample>,
        replay: bool,
    ) -> Result<RunReport, HarnessError> {
        self.run_task_in(task, playbook, replay, None).await
    }

    /// Run a task through the harness with a playbook; `workspace` overrides
    /// the configured one (packs prepare a fresh directory per task).
    pub(crate) async fn run_task_in(
        &self,
        task: &Task,
        playbook: Vec<PlaybookExample>,
        replay: bool,
        workspace: Option<&std::path::Path>,
    ) -> Result<RunReport, HarnessError> {
        let mut cfg = self.harness_cfg.clone();
        cfg.playbook = playbook;
        // Replays bypass the memo so both arms pay for every call and the
        // cost comparison is honest.
        cfg.no_memo = replay;
        if let Some(w) = workspace {
            cfg.workspace = w.to_path_buf();
        }
        let harness = Harness::new(self.store.clone(), cfg).await?;
        harness.run(&task.task, task.context.clone()).await
    }

    async fn judge(&self, task: &Task, report: &RunReport) -> Verdict {
        self.judge_in(task, report, &self.harness_cfg.workspace)
            .await
    }

    /// Judge a run's `FINAL` with the task's oracle in `workspace`.
    pub(crate) async fn judge_in(
        &self,
        task: &Task,
        report: &RunReport,
        workspace: &std::path::Path,
    ) -> Verdict {
        match &report.root.outcome {
            Outcome::Final { answer } => {
                let judge = self
                    .harness_cfg
                    .provider
                    .clone()
                    .map(|p| (p, self.cfg.judge_alias.clone()));
                task.verify.check(answer, workspace, judge).await
            }
            other => Verdict {
                pass: false,
                detail: format!("no FINAL: {}", other.tag()),
            },
        }
    }

    /// Run one task through the loop: pick (or take `task_id`), solve,
    /// judge, rate, learn. Returns `None` when nothing is pending.
    pub async fn step(
        &self,
        task_id: Option<&str>,
        generators: &[String],
    ) -> Result<Option<StepReport>, HarnessError> {
        let task = match task_id {
            Some(id) => self.task(id).await?,
            None => self.next_task(generators).await?,
        };
        let Some(task) = task else {
            return Ok(None);
        };
        self.store
            .execute(&format!(
                "UPDATE tasks SET status = 'running', updated_at = {} WHERE id = {}",
                now(),
                s(&task.id)
            ))
            .await?;
        let playbook = self.playbook_for(&task.kind).await?;
        let used_version = playbook.first().map(|p| p.version);
        let report = self.run_task(&task, playbook, false).await?;
        let verdict = self.judge(&task, &report).await;
        let usage = &report.root.usage;
        let depth = self.run_depth(&report.run.to_string()).await;
        let status = if verdict.pass {
            "solved"
        } else if matches!(task.verify, Verify::Human) {
            "needs_review"
        } else {
            "failed"
        };
        self.store
            .execute(&format!(
                "UPDATE tasks SET status = {}, attempts = attempts + 1, last_run = {}, last_detail = {}, updated_at = {} WHERE id = {}",
                s(status),
                s(&report.run.to_string()),
                s(&verdict.detail),
                now(),
                s(&task.id)
            ))
            .await?;
        self.store
            .execute(&format!(
                "INSERT INTO attempts VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
                s(&task.id),
                s(&report.run.to_string()),
                s(&self.solver),
                verdict.pass,
                s(&verdict.detail),
                usage.calls,
                usage.dollars,
                depth,
                used_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "NULL".into()),
                now()
            ))
            .await?;
        self.rate(&task, verdict.pass, usage.dollars).await?;
        self.update_generator(&task, verdict.pass).await?;
        if let Some(v) = used_version {
            self.store
                .execute(&format!(
                    "UPDATE playbook SET tries = tries + 1, wins = wins + {} WHERE version = {v}",
                    if verdict.pass { 1 } else { 0 }
                ))
                .await?;
        }
        let mut candidate = None;
        let mut adopted = None;
        if verdict.pass {
            if let Some(sql) = winning_sql(&report) {
                let v = self.record_candidate(&task, &sql, &report).await?;
                candidate = Some(v);
                adopted = Some(self.gate(&task.kind, v).await?);
            }
        }
        Ok(Some(StepReport {
            task: task.id.clone(),
            kind: task.kind.clone(),
            generator: task.generator.clone(),
            solved: verdict.pass,
            detail: verdict.detail,
            run: report.run.to_string(),
            calls: usage.calls,
            dollars: usage.dollars,
            playbook_candidate: candidate,
            adopted,
        }))
    }

    pub(crate) async fn run_depth(&self, run: &str) -> i64 {
        self.store
            .query(&format!(
                "SELECT COALESCE(MAX(depth), 0) FROM trace_sessions WHERE run = {}",
                s(run)
            ))
            .await
            .map(|b| int_at(&b, 0, 0))
            .unwrap_or(0)
    }

    pub(crate) async fn rate(
        &self,
        task: &Task,
        solved: bool,
        dollars: f64,
    ) -> Result<(), HarnessError> {
        let (solver, sgames) = self.solver_rating().await?;
        let b = self
            .store
            .query(&format!(
                "SELECT rating, games FROM task_ratings WHERE task = {}",
                s(&task.id)
            ))
            .await?;
        let (trating, tgames) = if b.rows.is_empty() {
            (task.difficulty, 0)
        } else {
            (float_at(&b, 0, 0), int_at(&b, 0, 1) as u64)
        };
        let (ns, nt) = ratings::update(solver, sgames, trating, tgames, solved);
        self.store
            .execute(&format!(
                "INSERT OR REPLACE INTO task_ratings VALUES ({}, {ns_t}, {})",
                s(&task.id),
                tgames + 1,
                ns_t = nt
            ))
            .await?;
        let b = self
            .store
            .query(&format!(
                "SELECT solved, dollars FROM solver_ratings WHERE solver = {}",
                s(&self.solver)
            ))
            .await?;
        let (solved_n, spent) = if b.rows.is_empty() {
            (0, 0.0)
        } else {
            (int_at(&b, 0, 0), float_at(&b, 0, 1))
        };
        self.store
            .execute(&format!(
                "INSERT OR REPLACE INTO solver_ratings VALUES ({}, {ns}, {}, {}, {})",
                s(&self.solver),
                sgames + 1,
                solved_n + if solved { 1 } else { 0 },
                spent + dollars
            ))
            .await?;
        Ok(())
    }

    async fn update_generator(&self, task: &Task, solved: bool) -> Result<(), HarnessError> {
        if task.generator == "user" {
            return Ok(());
        }
        let b = self
            .store
            .query(&format!(
                "SELECT dial, solve_rate, tried, solved FROM generator_state WHERE generator = {}",
                s(&task.generator)
            ))
            .await?;
        let (dial, rate, tried, solved_n) = if b.rows.is_empty() {
            (task.dial, 0.5, 0, 0)
        } else {
            (
                float_at(&b, 0, 0),
                float_at(&b, 0, 1),
                int_at(&b, 0, 2),
                int_at(&b, 0, 3),
            )
        };
        // Exponential moving average of the solve rate, then step the dial.
        let rate = 0.7 * rate + 0.3 * if solved { 1.0 } else { 0.0 };
        let dial = ratings::step_dial(dial, rate, self.cfg.dial_step);
        self.store
            .execute(&format!(
                "INSERT OR REPLACE INTO generator_state VALUES ({}, {dial}, {rate}, {}, {}, {})",
                s(&task.generator),
                tried + 1,
                solved_n + if solved { 1 } else { 0 },
                now()
            ))
            .await?;
        Ok(())
    }

    pub(crate) async fn record_candidate(
        &self,
        task: &Task,
        sql: &str,
        report: &RunReport,
    ) -> Result<i64, HarnessError> {
        let b = self
            .store
            .query("SELECT COALESCE(MAX(version), 0) + 1 FROM playbook")
            .await?;
        let version = int_at(&b, 0, 0);
        self.store
            .execute(&format!(
                "INSERT INTO playbook VALUES ({version}, {}, {}, 1, 1, {}, false, {}, NULL, {})",
                s(&task.kind),
                s(sql),
                report.root.usage.dollars,
                s(&report.run.to_string()),
                now()
            ))
            .await?;
        Ok(version)
    }

    /// Replay gate: re-run a sample of this kind's attempted tasks with the
    /// candidate as the only playbook entry and compare with the adopted
    /// set. The candidate is adopted when it solves at least as many at no
    /// more cost; the eval is recorded either way.
    pub async fn gate(&self, kind: &str, version: i64) -> Result<bool, HarnessError> {
        if self.cfg.replay_sample == 0 {
            self.adopt(version, "adopted without replay (replay_sample = 0)")
                .await?;
            return Ok(true);
        }
        let sample = self
            .store
            .query(&format!(
                "SELECT id FROM tasks WHERE kind = {} AND status IN ('solved', 'failed') ORDER BY updated_at DESC LIMIT {}",
                s(kind),
                self.cfg.replay_sample
            ))
            .await?;
        let ids: Vec<String> = (0..sample.rows.len())
            .map(|i| text_at(&sample, i, 0))
            .collect();
        let baseline = self.playbook_for(kind).await?;
        let candidate = self.playbook_entry(version).await?;
        let (mut b_solved, mut b_cost, mut c_solved, mut c_cost) = (0, 0.0, 0, 0.0);
        for id in &ids {
            let Some(task) = self.task(id).await? else {
                continue;
            };
            let br = self.run_task(&task, baseline.clone(), true).await?;
            if self.judge(&task, &br).await.pass {
                b_solved += 1;
            }
            b_cost += br.root.usage.dollars;
            let cr = self.run_task(&task, vec![candidate.clone()], true).await?;
            if self.judge(&task, &cr).await.pass {
                c_solved += 1;
            }
            c_cost += cr.root.usage.dollars;
        }
        let adopted = c_solved >= b_solved && c_cost <= b_cost + 1e-9;
        self.store
            .execute(&format!(
                "INSERT INTO playbook_evals VALUES ({}, {}, {version}, {b_solved}, {c_solved}, {}, {b_cost}, {c_cost}, {adopted})",
                now(),
                s(kind),
                ids.len()
            ))
            .await?;
        let note = format!(
            "replay of {} task(s): baseline {b_solved} solved for ${b_cost:.4}, candidate {c_solved} solved for ${c_cost:.4}",
            ids.len()
        );
        if adopted {
            self.adopt(version, &note).await?;
        } else {
            self.store
                .execute(&format!(
                    "UPDATE playbook SET eval_note = {} WHERE version = {version}",
                    s(&format!("rejected: {note}"))
                ))
                .await?;
        }
        Ok(adopted)
    }

    async fn playbook_entry(&self, version: i64) -> Result<PlaybookExample, HarnessError> {
        let b = self
            .store
            .query(&format!(
                "SELECT kind, sql, wins, tries FROM playbook WHERE version = {version}"
            ))
            .await?;
        if b.rows.is_empty() {
            return Err(HarnessError::Config(format!(
                "no playbook version {version}"
            )));
        }
        Ok(PlaybookExample {
            kind: text_at(&b, 0, 0),
            sql: text_at(&b, 0, 1),
            wins: int_at(&b, 0, 2) as u64,
            tries: int_at(&b, 0, 3) as u64,
            version,
        })
    }

    async fn adopt(&self, version: i64, note: &str) -> Result<(), HarnessError> {
        self.store
            .execute(&format!(
                "UPDATE playbook SET adopted = true, eval_note = {} WHERE version = {version}",
                s(note)
            ))
            .await?;
        // Failed user tasks of this kind get another chance now that the
        // harness has learned something relevant.
        self.store
            .execute(&format!(
                "UPDATE tasks SET status = 'pending', updated_at = {} WHERE status = 'failed' AND source = 'user' AND kind = (SELECT kind FROM playbook WHERE version = {version})",
                now()
            ))
            .await?;
        Ok(())
    }

    /// Withdraw a playbook version (`callgebra learn revert`).
    pub async fn revert(&self, version: i64) -> Result<(), HarnessError> {
        let _ = self.playbook_entry(version).await?;
        self.store
            .execute(&format!(
                "UPDATE playbook SET adopted = false, eval_note = {} WHERE version = {version}",
                s("reverted by hand")
            ))
            .await?;
        Ok(())
    }

    /// Keep every wanted generator's pending queue at `depth`.
    pub async fn refill(&self, generators: &[String], depth: usize) -> Result<usize, HarnessError> {
        let wanted: Vec<String> = if generators.is_empty() {
            GENERATORS.iter().map(|g| g.to_string()).collect()
        } else {
            generators.to_vec()
        };
        let mut made = 0;
        for g in wanted {
            let b = self
                .store
                .query(&format!(
                    "SELECT COUNT(*) FROM tasks WHERE generator = {} AND status = 'pending'",
                    s(&g)
                ))
                .await?;
            let pending = int_at(&b, 0, 0) as usize;
            if pending < depth {
                made += self.generate(&g, depth - pending).await?.len();
            }
        }
        Ok(made)
    }

    /// Unattended loop: refill, step, repeat until a limit is hit. Every
    /// piece of state is in the store, so stopping and starting again
    /// resumes.
    pub async fn run(&self, limits: &RunLimits) -> Result<Vec<StepReport>, HarnessError> {
        let started = std::time::Instant::now();
        let mut reports = vec![];
        let mut spent = 0.0;
        while reports.len() < limits.max_tasks {
            if limits.max_wall.is_some_and(|w| started.elapsed() >= w) {
                break;
            }
            if limits.max_dollars.is_some_and(|d| spent >= d) {
                break;
            }
            self.refill(&limits.generators, limits.queue_depth).await?;
            match self.step(None, &limits.generators).await? {
                Some(r) => {
                    spent += r.dollars;
                    reports.push(r);
                }
                None => break,
            }
        }
        Ok(reports)
    }

    /// The board: counts per generator and status, plus each generator's dial.
    pub async fn board(&self) -> Result<Batch, HarnessError> {
        Ok(self
            .store
            .query(
                "SELECT t.generator, COALESCE(g.dial, 0.3) AS dial, \
                 SUM(CASE WHEN t.status = 'pending' THEN 1 ELSE 0 END) AS pending, \
                 SUM(CASE WHEN t.status = 'running' THEN 1 ELSE 0 END) AS running, \
                 SUM(CASE WHEN t.status = 'solved' THEN 1 ELSE 0 END) AS solved, \
                 SUM(CASE WHEN t.status = 'failed' THEN 1 ELSE 0 END) AS failed, \
                 SUM(CASE WHEN t.status = 'needs_review' THEN 1 ELSE 0 END) AS review \
                 FROM tasks t LEFT JOIN generator_state g ON g.generator = t.generator \
                 GROUP BY t.generator, g.dial ORDER BY t.generator",
            )
            .await?)
    }

    /// The morning query from the plan.
    pub async fn report(&self) -> Result<Batch, HarnessError> {
        Ok(self
            .store
            .query(
                "SELECT generator, ROUND(difficulty) AS difficulty, AVG(solved) AS solved, AVG(calls) AS calls, AVG(depth) AS depth, COUNT(*) AS attempts FROM trace_tasks GROUP BY 1, 2 ORDER BY 1, 2",
            )
            .await?)
    }

    /// Per-kind playbook summary.
    pub async fn playbook(&self) -> Result<Batch, HarnessError> {
        Ok(self
            .store
            .query("SELECT version, kind, adopted, wins, tries, ROUND(avg_cost, 4) AS avg_cost, eval_note, sql FROM playbook ORDER BY version")
            .await?)
    }
}

/// The SQL that produced a solved task's `FINAL`: every statement of the root
/// transcript that did not error, joined in order. That is the playbook
/// candidate.
pub(crate) fn winning_sql(report: &RunReport) -> Option<String> {
    let mut parts = vec![];
    for turn in &report.root.transcript {
        if let Some(sql) = &turn.sql {
            if turn.results.iter().any(|r| r.is_error) {
                continue;
            }
            parts.push(sql.trim().to_string());
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(24);
    out.trim_matches('_').to_string()
}

fn uuid_seed() -> u64 {
    let id = callgebra_core::RunId::new().to_string();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
