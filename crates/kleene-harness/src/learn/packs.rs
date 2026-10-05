//! Task packs: directories under `tasks/` holding a `pack.json` with a list
//! of tasks, each with its oracle. Packs are how external benchmarks enter
//! the loop (OOLONG-like corpora, a Terminal-Bench-style subset, finance and
//! legal streams, Harvey LAB imports), and how generated streams are frozen
//! so a run is reproducible.

use super::generators::{self, GeneratedTask};
use super::verify::Verify;
use crate::HarnessError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A pack on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    /// Pack name (directory name by convention).
    pub name: String,
    /// One line on what it measures.
    pub description: String,
    /// Where the tasks came from and under what terms.
    #[serde(default)]
    pub license: String,
    /// Default task kind for playbook lookup.
    pub kind: String,
    /// The tasks, in stream order.
    pub tasks: Vec<PackTask>,
}

/// One task in a pack.
///
/// On disk a task is either written out in full, or names a generator, a
/// dial and a seed in `from` and is regenerated on load (`bench build
/// --lazy`); the two forms load to the same task, because every generator
/// is a pure function of its dial and seed. A task with `from` is saved in
/// the short form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PackTaskOnDisk", into = "PackTaskOnDisk")]
pub struct PackTask {
    /// Stable id within the pack.
    pub id: String,
    /// Kind override.
    pub kind: Option<String>,
    /// Task text.
    pub task: String,
    /// Inline context (one paragraph per `ctx` row, blank-line separated).
    pub context: Option<String>,
    /// Context read from a file relative to the pack directory.
    pub context_file: Option<String>,
    /// Shell commands run in the workspace before the task (Terminal-Bench
    /// style setup). For a task that `continues` another they run in the
    /// inherited workspace, so they can check or repair what the earlier
    /// step left.
    pub setup: Vec<String>,
    /// A directory (relative to the pack) copied into the workspace before
    /// setup, for document-heavy tasks (LAB matter folders).
    pub workspace_from: Option<String>,
    /// The id of the task whose workspace this one starts from: a step in
    /// a multi-step episode. The workspace that task left, files and all,
    /// is this task's workspace; `setup` then runs in it. The task named
    /// must come earlier in the pack.
    pub continues: Option<String>,
    /// A shell command (the task's visible tests, say) that must exit 0
    /// before the solver's `FINAL` is accepted; a failing check is rendered
    /// back and the solver continues. The hidden oracle in `verify` is still
    /// what grades the task.
    pub check: Option<String>,
    /// The oracle.
    pub verify: Verify,
    /// Difficulty prior in rating points.
    pub difficulty: f64,
    /// The generator this task was regenerated from, when it was.
    pub from: Option<GeneratorRef>,
}

/// A task that is regenerated on load: a generator with its dial and seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratorRef {
    /// Generator name (`generators::GENERATORS`).
    pub generator: String,
    /// Hardness dial.
    pub dial: f64,
    /// Seed.
    pub seed: u64,
}

/// The on-disk shape of a task: the full form, or `from` plus overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackTaskOnDisk {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<GeneratorRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_file: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    setup: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continues: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verify: Option<Verify>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    difficulty: Option<f64>,
}

impl TryFrom<PackTaskOnDisk> for PackTask {
    type Error = String;

    fn try_from(d: PackTaskOnDisk) -> Result<Self, String> {
        let generated = match &d.from {
            Some(r) => Some(
                generators::generate(&r.generator, r.dial, r.seed, &std::env::temp_dir())
                    .ok_or_else(|| format!("task {}: unknown generator {}", d.id, r.generator))?,
            ),
            None => None,
        };
        let (kind, task, context, verify, difficulty) = match generated {
            Some(g) => (
                d.kind.or(Some(g.kind)),
                d.task.unwrap_or(g.task),
                d.context.or(g.context),
                d.verify.unwrap_or(g.verify),
                d.difficulty.unwrap_or(g.difficulty),
            ),
            None => (
                d.kind,
                d.task
                    .ok_or_else(|| format!("task {}: missing field `task`", d.id))?,
                d.context,
                d.verify
                    .ok_or_else(|| format!("task {}: missing field `verify`", d.id))?,
                d.difficulty.unwrap_or(super::ratings::BASELINE),
            ),
        };
        Ok(PackTask {
            id: d.id,
            kind,
            task,
            context,
            context_file: d.context_file,
            setup: d.setup,
            workspace_from: d.workspace_from,
            continues: d.continues,
            check: d.check,
            verify,
            difficulty,
            from: d.from,
        })
    }
}

impl From<PackTask> for PackTaskOnDisk {
    fn from(t: PackTask) -> Self {
        let lazy = t.from.is_some();
        PackTaskOnDisk {
            id: t.id,
            kind: t.kind,
            from: t.from,
            task: (!lazy).then_some(t.task),
            context: if lazy { None } else { t.context },
            context_file: t.context_file,
            setup: t.setup,
            workspace_from: t.workspace_from,
            continues: t.continues,
            check: t.check,
            verify: (!lazy).then_some(t.verify),
            difficulty: (!lazy).then_some(t.difficulty),
        }
    }
}

impl Pack {
    /// Read `dir/pack.json`.
    pub fn load(dir: &Path) -> Result<Self, HarnessError> {
        let path = dir.join("pack.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| HarnessError::Config(format!("cannot read {}: {e}", path.display())))?;
        let mut pack: Pack = serde_json::from_str(&text)
            .map_err(|e| HarnessError::Config(format!("{} is not a pack: {e}", path.display())))?;
        if pack.name.is_empty() {
            pack.name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        pack.check()?;
        Ok(pack)
    }

    /// Structural checks: ids are unique, and a task that `continues`
    /// another names an earlier task of this pack.
    pub fn check(&self) -> Result<(), HarnessError> {
        let mut seen: Vec<&str> = vec![];
        for t in &self.tasks {
            if seen.contains(&t.id.as_str()) {
                return Err(HarnessError::Config(format!(
                    "pack {}: duplicate task id {}",
                    self.name, t.id
                )));
            }
            if let Some(prev) = &t.continues {
                if !seen.contains(&prev.as_str()) {
                    return Err(HarnessError::Config(format!(
                        "pack {}: task {} continues {}, which is not an earlier task",
                        self.name, t.id, prev
                    )));
                }
            }
            seen.push(&t.id);
        }
        Ok(())
    }

    /// The chain of tasks a task's workspace descends from, root first and
    /// the task itself last.
    pub fn lineage<'a>(&'a self, t: &'a PackTask) -> Vec<&'a PackTask> {
        let mut chain = vec![t];
        let mut cur = t;
        while let Some(prev) = &cur.continues {
            match self.tasks.iter().find(|x| &x.id == prev) {
                Some(p) => {
                    chain.push(p);
                    cur = p;
                }
                None => break,
            }
        }
        chain.reverse();
        chain
    }

    /// Write `dir/pack.json` (and nothing else).
    pub fn save(&self, dir: &Path) -> Result<(), HarnessError> {
        std::fs::create_dir_all(dir)
            .map_err(|e| HarnessError::Config(format!("cannot create {}: {e}", dir.display())))?;
        let text =
            serde_json::to_string_pretty(self).map_err(|e| HarnessError::Config(e.to_string()))?;
        std::fs::write(dir.join("pack.json"), text)
            .map_err(|e| HarnessError::Config(format!("cannot write pack: {e}")))?;
        Ok(())
    }

    /// Resolve a task's context (inline or from a file).
    pub fn context_of(&self, dir: &Path, t: &PackTask) -> Result<Option<String>, HarnessError> {
        if let Some(c) = &t.context {
            return Ok(Some(c.clone()));
        }
        if let Some(f) = &t.context_file {
            let p = dir.join(f);
            return std::fs::read_to_string(&p)
                .map(Some)
                .map_err(|e| HarnessError::Config(format!("cannot read {}: {e}", p.display())));
        }
        Ok(None)
    }

    /// Freeze `count` tasks from a generator at `dial` into a pack, seeds
    /// `seed..seed + count`, so the stream is reproducible. A `lazy` pack
    /// stores only the generator references and regenerates on load, which
    /// keeps a pack with long contexts small on disk.
    pub fn from_generator(
        name: &str,
        generator: &str,
        count: usize,
        dial: f64,
        seed: u64,
        workspace: &Path,
        lazy: bool,
    ) -> Result<Self, HarnessError> {
        if !generators::GENERATORS.contains(&generator) {
            return Err(HarnessError::Config(format!(
                "unknown generator {generator}"
            )));
        }
        let mut tasks = vec![];
        let mut kind = String::new();
        for i in 0..count as u64 {
            let Some(g) = generators::generate(generator, dial, seed + i, workspace) else {
                continue;
            };
            kind = g.kind.clone();
            tasks.push(PackTask {
                id: format!("{generator}-{:03}", i + 1),
                // Generators like `logbook` vary the kind with the seed, so
                // every task carries its own.
                kind: Some(g.kind.clone()),
                task: g.task,
                context: g.context,
                context_file: None,
                setup: vec![],
                workspace_from: None,
                continues: None,
                check: None,
                verify: g.verify,
                difficulty: g.difficulty,
                from: lazy.then(|| GeneratorRef {
                    generator: generator.to_string(),
                    dial,
                    seed: seed + i,
                }),
            });
        }
        Ok(Pack {
            name: name.to_string(),
            description: format!(
                "{count} tasks frozen from the `{generator}` generator at dial {dial} (seeds {seed}..{})",
                seed + count as u64
            ),
            license: "synthetic, generated by kleene".into(),
            kind,
            tasks,
        })
    }

    /// Turn a pack task into the loop's task shape.
    pub fn to_generated(&self, dir: &Path, t: &PackTask) -> Result<GeneratedTask, HarnessError> {
        Ok(GeneratedTask {
            generator: format!("pack:{}", self.name),
            kind: t.kind.clone().unwrap_or_else(|| self.kind.clone()),
            task: t.task.clone(),
            context: self.context_of(dir, t)?,
            verify: t.verify.clone(),
            dial: 0.5,
            difficulty: t.difficulty,
        })
    }
}

/// A fresh directory under the system temp dir, removed on drop.
pub struct Workspace {
    path: PathBuf,
}

impl Workspace {
    /// A fresh empty directory under the system temp dir, removed on drop.
    pub(crate) fn create() -> Result<Self, HarnessError> {
        let path = std::env::temp_dir().join(format!("kleene-ws-{}", kleene_core::RunId::new()));
        std::fs::create_dir_all(&path)
            .map_err(|e| HarnessError::Config(format!("cannot create a workspace: {e}")))?;
        Ok(Self { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Prepare a task's workspace: a fresh temporary directory, optionally
/// seeded from `workspace_from`, then the setup commands. Returns the
/// directory (kept alive by the caller).
pub async fn prepare_workspace(pack_dir: &Path, t: &PackTask) -> Result<Workspace, HarnessError> {
    let dir = Workspace::create()?;
    if let Some(from) = &t.workspace_from {
        copy_dir(&pack_dir.join(from), dir.path())?;
    }
    run_setup(&dir, t).await?;
    Ok(dir)
}

/// Run a task's setup commands in an existing workspace: the second and
/// later steps of an episode run theirs in the workspace the previous step
/// left.
pub async fn run_setup(dir: &Workspace, t: &PackTask) -> Result<(), HarnessError> {
    for cmd in &t.setup {
        let out = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir.path())
            .output()
            .await
            .map_err(|e| HarnessError::Config(format!("setup failed to start: {e}")))?;
        if !out.status.success() {
            return Err(HarnessError::Config(format!(
                "setup `{cmd}` failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), HarnessError> {
    let err = |e: std::io::Error| HarnessError::Config(format!("copy {}: {e}", from.display()));
    std::fs::create_dir_all(to).map_err(err)?;
    for entry in std::fs::read_dir(from).map_err(err)? {
        let entry = entry.map_err(err)?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(err)?;
        }
    }
    Ok(())
}

/// The Terminal-Bench-style subset: shell tasks in a fresh workspace, each
/// checked by a shell oracle over the resulting files. Small, but it covers
/// the whole SQL-mapped shell surface: `CALL shell`, `write_file`, `files`,
/// `read`, `grep`, and `FINAL`.
pub fn terminal_pack() -> Pack {
    let t = |id: &str, task: &str, setup: &[&str], check: &str| PackTask {
        id: id.into(),
        kind: None,
        task: task.into(),
        context: None,
        context_file: None,
        setup: setup.iter().map(|s| s.to_string()).collect(),
        workspace_from: None,
        continues: None,
        check: None,
        verify: Verify::Shell {
            command: check.into(),
        },
        difficulty: 1000.0,
        from: None,
    };
    Pack {
        name: "terminal".into(),
        description: "Terminal-Bench-style shell tasks in a fresh workspace, judged by shell oracles over the files left behind".into(),
        license: "original tasks, MIT".into(),
        kind: "terminal".into(),
        tasks: vec![
            t(
                "create-file",
                "Create a file named hello.txt in the workspace containing exactly the text 'hello, world' (no trailing newline is fine). Then answer with FINAL over one row with column done = true.",
                &[],
                "test -f hello.txt && grep -q 'hello, world' hello.txt",
            ),
            t(
                "count-lines",
                "The workspace has a file data.txt. How many lines does it have? Answer with FINAL over one row with column lines (an integer).",
                &["printf 'a\\nb\\nc\\nd\\ne\\n' > data.txt"],
                "grep -q '\"5\"' -",
            ),
            t(
                "rename-extension",
                "Rename every .log file in the workspace to .txt (same base name). Then answer with FINAL over one row with column renamed = the number of files renamed.",
                &["touch a.log b.log c.log", "echo keep > keep.md"],
                "test -f a.txt && test -f b.txt && test -f c.txt && ! ls *.log 2>/dev/null && test -f keep.md",
            ),
            t(
                "grep-count",
                "Count how many lines in notes.md mention the word TODO (case-insensitive). Answer with FINAL over one row with column todos (an integer).",
                &["printf 'TODO: x\\ndone\\ntodo: y\\nnothing\\nToDo z\\n' > notes.md"],
                "grep -q '\"3\"' -",
            ),
            t(
                "sum-column",
                "The workspace has sales.csv with a header row and columns region,amount. What is the total of the amount column? Answer with FINAL over one row with column total.",
                &["printf 'region,amount\\nnorth,10\\nsouth,25\\neast,5\\n' > sales.csv"],
                "grep -q '\"40\"' -",
            ),
            t(
                "write-json",
                "Write a file config.json in the workspace containing a JSON object with keys name (value \"kleene\") and version (value 7). Answer with FINAL over one row with column done = true.",
                &[],
                "python3 -c \"import json,sys; d=json.load(open('config.json')); sys.exit(0 if d.get('name')=='kleene' and d.get('version')==7 else 1)\"",
            ),
        ],
    }
}

/// Import a Harvey LAB checkout (`harveyai/harvey-labs`, MIT): every
/// directory with a `task.json` becomes a task whose workspace is the
/// matter folder (`documents/`, or the shared corpus a `docs_dir` points
/// at, copied once and shared) and whose oracle is a judge over the task's
/// rubric criteria, with deliverables expected under `output/`. LAB's own
/// evaluator (`lab_core.evaluation.run_eval`, two judges, all-pass) remains
/// the scorer of record; this oracle is the in-loop approximation.
///
/// The schema read is LAB's as shipped: `instructions`, `title`,
/// `work_type`, `deliverables` (a map from file name to description, or a
/// list), `criteria` (objects with `id`, `title`, `match_criteria` and the
/// `deliverables` they are scoped to), and `docs_dir` for tasks over a
/// shared corpus.
pub fn import_lab(
    root: &Path,
    pack_dir: &Path,
    sample: Option<(usize, u64)>,
) -> Result<Pack, HarnessError> {
    let mut tasks = vec![];
    let mut dirs = vec![];
    walk_for(root, "task.json", &mut dirs, 0);
    dirs.sort();
    if let Some((k, seed)) = sample {
        let keep = sample_indices(dirs.len(), k, seed);
        dirs = keep.into_iter().map(|i| dirs[i].clone()).collect();
    }
    // Shared corpora (`docs_dir`) are copied once; canonical source path to
    // the workspace directory, relative to the pack.
    let mut shared: std::collections::BTreeMap<PathBuf, String> = Default::default();
    for dir in dirs {
        let text = std::fs::read_to_string(dir.join("task.json"))
            .map_err(|e| HarnessError::Config(format!("{}: {e}", dir.display())))?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| HarnessError::Config(format!("{}: not JSON: {e}", dir.display())))?;
        let field = |k: &str| {
            json.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let id = dir
            .strip_prefix(root)
            .unwrap_or(&dir)
            .display()
            .to_string()
            .replace(['/', '\\'], "__");
        let deliverables = lab_deliverables(&json);
        let criteria = lab_criteria(&json);
        let rubric = if criteria.is_empty() {
            format!(
                "Every requirement of the instructions is met and every deliverable ({}) was written under output/.",
                deliverables.join(", ")
            )
        } else {
            format!(
                "All of the following must hold (all-pass), judged against the files under output/:\n- {}",
                criteria.join("\n- ")
            )
        };
        // The matter documents: the task's own documents/ folder, or a shared
        // corpus named by docs_dir (relative to the task directory).
        let docs_dir = field("docs_dir");
        let ws_rel = if docs_dir.is_empty() {
            let ws_rel = format!("workspaces/{id}");
            let ws = pack_dir.join(&ws_rel);
            std::fs::create_dir_all(&ws).map_err(|e| {
                HarnessError::Config(format!("cannot create {}: {e}", ws.display()))
            })?;
            let docs = dir.join("documents");
            if docs.is_dir() {
                copy_dir(&docs, &ws.join("documents"))?;
            }
            ws_rel
        } else {
            let src = dir.join(&docs_dir);
            let key = src.canonicalize().unwrap_or(src.clone());
            if let Some(rel) = shared.get(&key) {
                rel.clone()
            } else {
                let rel = format!("workspaces/shared-{:02}", shared.len() + 1);
                copy_dir(&src, &pack_dir.join(&rel).join("documents"))?;
                shared.insert(key, rel.clone());
                rel
            }
        };
        tasks.push(PackTask {
            id: id.clone(),
            kind: Some(format!(
                "lab_{}",
                if field("work_type").is_empty() {
                    "task".to_string()
                } else {
                    field("work_type")
                }
            )),
            task: format!(
                "{}\n\nThe matter documents are under documents/ in the workspace (use files, read and grep). Write every deliverable under output/ with CALL write_file{}. Finish with FINAL over one row per deliverable: columns path and summary.",
                field("instructions"),
                if deliverables.is_empty() {
                    String::new()
                } else {
                    format!(" (expected: {})", deliverables.join(", "))
                }
            ),
            context: None,
            context_file: None,
            setup: vec!["mkdir -p output".into()],
            workspace_from: Some(ws_rel),
            continues: None,
            check: None,
            verify: Verify::Judge {
                rubric,
                reference: None,
            },
            difficulty: 1400.0,
            from: None,
        });
    }
    if tasks.is_empty() {
        return Err(HarnessError::Config(format!(
            "no task.json found under {}",
            root.display()
        )));
    }
    Ok(Pack {
        name: "harvey-lab".into(),
        description: "Harvey Legal Agent Benchmark tasks imported from a checkout; matter folders as workspaces, deliverables under output/, judge oracle over the task criteria (LAB's evaluation.run_eval is the scorer of record)".into(),
        license: "Harvey LAB, MIT (the imported documents keep their own terms)".into(),
        kind: "lab_task".into(),
        tasks,
    })
}

/// `k` of `n` indices chosen without replacement by a seeded shuffle, in
/// ascending order, so a sample of a pack keeps the pack's order and the
/// same seed gives the same sample on every machine.
pub fn sample_indices(n: usize, k: usize, seed: u64) -> Vec<usize> {
    let mut rng = generators::Rng::new(seed ^ 0x5eed_5eed_5eed_5eed);
    let mut idx: Vec<usize> = (0..n).collect();
    let k = k.min(n);
    for i in 0..k {
        let j = i + rng.below((n - i) as u64) as usize;
        idx.swap(i, j);
    }
    let mut out = idx[..k].to_vec();
    out.sort_unstable();
    out
}

impl Pack {
    /// The pack with `k` tasks sampled by `seed` (see [`sample_indices`]),
    /// in their original order; the name is kept so evals group with the
    /// full pack.
    pub fn sampled(&self, k: usize, seed: u64) -> Pack {
        let keep = sample_indices(self.tasks.len(), k, seed);
        Pack {
            tasks: keep.into_iter().map(|i| self.tasks[i].clone()).collect(),
            ..self.clone()
        }
    }
}

/// The deliverable file names of a LAB task: `deliverables` is a map from
/// file name to description in the shipped tasks; older drafts used a list
/// of names or of `{filename}` objects.
fn lab_deliverables(json: &serde_json::Value) -> Vec<String> {
    let v = json
        .get("deliverables")
        .or_else(|| json.get("expected_deliverables"));
    match v {
        Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|d| {
                d.as_str().map(str::to_string).or_else(|| {
                    d.get("filename")
                        .and_then(|f| f.as_str())
                        .map(str::to_string)
                })
            })
            .collect(),
        _ => vec![],
    }
}

/// The rubric lines of a LAB task, one per criterion: `id`, `title`, the
/// `match_criteria` the judge checks, and the deliverables it is scoped to.
fn lab_criteria(json: &serde_json::Value) -> Vec<String> {
    let Some(a) = json.get("criteria").and_then(|v| v.as_array()) else {
        return vec![];
    };
    a.iter()
        .filter_map(|c| {
            if let Some(s) = c.as_str() {
                return Some(s.to_string());
            }
            let get = |k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").trim();
            let text = if get("match_criteria").is_empty() {
                get("text")
            } else {
                get("match_criteria")
            };
            if text.is_empty() {
                return None;
            }
            let mut line = String::new();
            if !get("id").is_empty() {
                line.push_str(get("id"));
                line.push(' ');
            }
            if !get("title").is_empty() {
                line.push_str(get("title"));
                line.push_str(": ");
            }
            line.push_str(text);
            let scoped: Vec<&str> = c
                .get("deliverables")
                .and_then(|d| d.as_array())
                .map(|d| d.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            if !scoped.is_empty() {
                line.push_str(&format!(" (in {})", scoped.join(", ")));
            }
            Some(line)
        })
        .collect()
}

/// One OOLONG-synth question as the Hugging Face dataset
/// `oolongbench/oolong-synth` stores it: the fields the importer reads.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OolongRow {
    /// Question id in the dataset.
    pub id: i64,
    /// Context length bucket in tokens (1024 to 4194304).
    pub context_len: i64,
    /// Source classification dataset (`trec_coarse`, `spam`, `agnews`, …).
    pub dataset: String,
    /// The context window: instructions, one entry per line, a reminder.
    pub context_window_text: String,
    /// The question, including the dataset's own answer-format line.
    pub question: String,
    /// `counting`, `user` or `timeline`.
    pub task_group: String,
    /// The fine task type (`TASK_TYPE.MOST_FREQ`, …).
    pub task: String,
    /// The expected answer as a Python list literal (`['correct']`, `[1542]`).
    pub answer: String,
    /// `ANSWER_TYPE.LABEL`, `ANSWER_TYPE.NUMERIC`, `ANSWER_TYPE.COMPARISON`, …
    pub answer_type: String,
    /// Which context window the question is over; questions share windows.
    pub context_window_id: i64,
}

/// Parse an OOLONG answer: a Python list literal (`['a', 'b']`, `[1542]`)
/// or a bare value, into its values.
pub fn oolong_answer_values(answer: &str) -> Vec<String> {
    let t = answer.trim();
    let inner = t
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(t);
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (None, ',') => {
                out.push(std::mem::take(&mut cur));
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Build a pack from OOLONG-synth rows (Bertsch et al. 2025, MIT). Every
/// context window is written once to `contexts/window-<id>.txt` under
/// `pack_dir`, one line of the original per `ctx` row, and every question
/// over it becomes a task with the `oolong` oracle. The rows keep their
/// order, so the same rows give the same pack; the context text is the
/// dataset's own (instructions, entries, reminder), untouched apart from
/// the row split.
pub fn oolong_pack(name: &str, rows: &[OolongRow], pack_dir: &Path) -> Result<Pack, HarnessError> {
    if rows.is_empty() {
        return Err(HarnessError::Config("no OOLONG rows to import".into()));
    }
    let ctx_dir = pack_dir.join("contexts");
    std::fs::create_dir_all(&ctx_dir)
        .map_err(|e| HarnessError::Config(format!("cannot create {}: {e}", ctx_dir.display())))?;
    let mut tasks = vec![];
    let mut written = std::collections::BTreeSet::new();
    for r in rows {
        let rel = format!("contexts/window-{}.txt", r.context_window_id);
        if written.insert(r.context_window_id) {
            let lines: Vec<&str> = r
                .context_window_text
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.trim().is_empty())
                .collect();
            std::fs::write(pack_dir.join(&rel), lines.join("\n\n"))
                .map_err(|e| HarnessError::Config(format!("cannot write {rel}: {e}")))?;
        }
        let numeric = r.answer_type.ends_with("NUMERIC");
        let answer = oolong_answer_values(&r.answer);
        let group = r.task_group.to_ascii_lowercase();
        // 1000 at 1k tokens, +100 per doubling: 1700 at 128k, 2200 at 4M.
        let difficulty = 1000.0 + 100.0 * ((r.context_len.max(1024) as f64) / 1024.0).log2();
        tasks.push(PackTask {
            id: format!("{}-{}", r.dataset, r.id),
            kind: Some(format!("oolong_{}_{group}", r.dataset)),
            task: format!(
                "{}\n\nThe data is in ctx, one line of the original per row in order: the dataset's instructions first, then one entry per row (`Date: … || User: … || Instance: …`), then a closing reminder. Answer with FINAL over one row and one column answer holding only the value (a label, a number, a date, a user id or a comparison; no `Answer:` or `Label:` prefix), or one row per value if the question asks for several.",
                r.question.trim()
            ),
            context: None,
            context_file: Some(rel),
            setup: vec![],
            workspace_from: None,
            verify: Verify::Oolong { answer, numeric },
            difficulty,
            continues: None,
            from: None,
        });
    }
    let datasets: std::collections::BTreeSet<&str> =
        rows.iter().map(|r| r.dataset.as_str()).collect();
    let lens: std::collections::BTreeSet<i64> = rows.iter().map(|r| r.context_len).collect();
    let kind = if datasets.len() == 1 {
        format!("oolong_{}", rows[0].dataset)
    } else {
        "oolong".to_string()
    };
    Ok(Pack {
        name: name.to_string(),
        description: format!(
            "OOLONG-synth {} at {} tokens: {} questions over {} context windows, imported from Hugging Face oolongbench/oolong-synth",
            datasets.iter().copied().collect::<Vec<_>>().join(", "),
            lens.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", "),
            tasks.len(),
            written.len()
        ),
        license: "OOLONG (Bertsch et al. 2025), MIT, github.com/abertsch72/oolong; downloaded on import, not redistributed".into(),
        kind,
        tasks,
    })
}

/// One example of the UmaiTech `legal-contract-*-redlining-*` datasets in
/// their `alpaca` config: an instruction, an input naming the clause's
/// category, contract type and jurisdiction and quoting the original
/// clause, and the reference output (redlined clause, rationale, specific
/// changes).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RedlineRow {
    /// The instruction ("Redline this contract clause to protect the client's interests…").
    #[serde(default)]
    pub instruction: String,
    /// `Clause Category: …\nContract Type: …\nJurisdiction: …\n\nOriginal Clause:\n…`.
    pub input: String,
    /// `Redlined Clause:\n…\n\nRationale:\n…\n\nSpecific Changes:\n- …`.
    pub output: String,
    /// Clause category, contract type, jurisdiction and risk reduction.
    #[serde(default)]
    pub metadata: RedlineMeta,
}

/// The metadata the redlining datasets attach to every example.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct RedlineMeta {
    /// CUAD clause category (`liability`, `termination`, `governing law`, …).
    #[serde(default)]
    pub clause_category: String,
    /// `employment_agreement`, `nda`, `license_agreement`, …
    #[serde(default)]
    pub contract_type: String,
    /// The client's jurisdiction (a US state).
    #[serde(default)]
    pub jurisdiction: String,
    /// `low`, `medium` or `high`.
    #[serde(default)]
    pub risk_reduction: String,
}

/// The parts of a redlining example the oracle and the task text need.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RedlineParts {
    /// The clause as drafted.
    pub original: String,
    /// The reference redline.
    pub redline: String,
    /// The reference rationale.
    pub rationale: String,
    /// The specific changes the reference lists, one per entry.
    pub changes: Vec<String>,
}

/// The text after `heading` up to the next blank-line-separated heading
/// among `stops`, trimmed.
fn section<'a>(text: &'a str, heading: &str, stops: &[&str]) -> Option<&'a str> {
    let start = text.find(heading)? + heading.len();
    let rest = &text[start..];
    let end = stops
        .iter()
        .filter_map(|h| rest.find(h))
        .min()
        .unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Split a row into its original clause, reference redline, rationale and
/// changes. Rows whose input or output lack the clause are an error, since
/// a task without a clause cannot be set.
pub fn redline_parts(row: &RedlineRow) -> Result<RedlineParts, HarnessError> {
    let original = section(&row.input, "Original Clause:", &[])
        .filter(|s| !s.is_empty())
        .ok_or_else(|| HarnessError::Config("a redlining row has no Original Clause".into()))?;
    let stops = ["Rationale:", "Specific Changes:", "Risk Reduction:"];
    let redline = section(&row.output, "Redlined Clause:", &stops)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| HarnessError::Config("a redlining row has no Redlined Clause".into()))?;
    let rationale = section(
        &row.output,
        "Rationale:",
        &["Specific Changes:", "Risk Reduction:"],
    )
    .unwrap_or_default();
    let changes = section(&row.output, "Specific Changes:", &["Risk Reduction:"])
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Ok(RedlineParts {
        original: original.to_string(),
        redline: redline.to_string(),
        rationale: rationale.to_string(),
        changes,
    })
}

/// Build a pack from redlining rows (the UmaiTech `legal-contract-*`
/// datasets, CC BY 4.0, derived from CUAD). Every row becomes a task with
/// the clause in the task text and the `redline` oracle holding the
/// reference; the rows keep their order, so the same rows give the same
/// pack. `source` names the dataset for the description and license line.
pub fn redlining_pack(name: &str, source: &str, rows: &[RedlineRow]) -> Result<Pack, HarnessError> {
    if rows.is_empty() {
        return Err(HarnessError::Config("no redlining rows to import".into()));
    }
    let slug = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    };
    let mut tasks = Vec::with_capacity(rows.len());
    let mut categories = std::collections::BTreeSet::new();
    for (i, r) in rows.iter().enumerate() {
        let parts = redline_parts(r)?;
        let m = &r.metadata;
        let category = if m.clause_category.is_empty() {
            "clause".to_string()
        } else {
            m.clause_category.clone()
        };
        categories.insert(category.clone());
        let words = parts.original.split_whitespace().count() as f64;
        // Longer clauses and more changes are harder: 1200 for a one-line
        // clause with one change, +100 per doubling of either.
        let difficulty = 1200.0
            + 100.0 * (words.max(16.0) / 16.0).log2()
            + 100.0 * (parts.changes.len().max(1) as f64).log2();
        let risk = if m.risk_reduction.is_empty() {
            String::new()
        } else {
            format!("Expected risk reduction: {}.\n", m.risk_reduction)
        };
        tasks.push(PackTask {
            id: format!("{:04}-{}", i + 1, slug(&category)),
            kind: Some(format!("redline_{}", slug(&category).replace('-', "_"))),
            task: format!(
                "Redline this {category} clause from a {contract} to protect the client's interests, under {jurisdiction} law. Identify the risks to the client and revise the clause to remove them, keeping it a complete, usable clause.\n{risk}\nOriginal clause:\n{clause}\n\nAnswer with FINAL over one row and two columns: redline (the full revised clause text, not a diff or commentary) and rationale (why each change protects the client).",
                contract = m.contract_type.replace('_', " "),
                jurisdiction = if m.jurisdiction.is_empty() { "the client's" } else { m.jurisdiction.as_str() },
                clause = parts.original,
            ),
            context: None,
            context_file: None,
            setup: vec![],
            workspace_from: None,
            verify: Verify::Redline {
                original: parts.original,
                redline: parts.redline,
                rationale: parts.rationale,
                changes: parts.changes,
            },
            difficulty,
            continues: None,
            from: None,
        });
    }
    Ok(Pack {
        name: name.to_string(),
        description: format!(
            "Contract redlining: {} clauses ({}) from {source}, each revised for the client against a GPT-generated reference redline",
            tasks.len(),
            categories.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
        license: format!(
            "{source} (UmaiTech, CC BY 4.0; synthetic redlines over CUAD, Hendrycks et al. 2021, CC BY 4.0); downloaded on import, not redistributed"
        ),
        kind: "redline".into(),
        tasks,
    })
}

fn walk_for(dir: &Path, file: &str, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    if dir.join(file).is_file() {
        out.push(dir.to_path_buf());
    }
    for e in entries {
        if e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
            walk_for(&e.path(), file, out, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_round_trip_and_generator_packs_are_reproducible() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::env::temp_dir();
        let a = Pack::from_generator("corp", "corpus", 3, 0.4, 11, &ws, false).unwrap();
        let b = Pack::from_generator("corp", "corpus", 3, 0.4, 11, &ws, false).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.tasks.len(), 3);
        a.save(dir.path()).unwrap();
        let loaded = Pack::load(dir.path()).unwrap();
        assert_eq!(loaded, a);
        let g = loaded.to_generated(dir.path(), &loaded.tasks[0]).unwrap();
        assert_eq!(g.generator, "pack:corp");
        assert_eq!(g.kind, "corpus_hours_by_project");
    }

    #[test]
    fn lazy_packs_store_generator_references_and_load_the_same_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let ws = std::env::temp_dir();
        let full = Pack::from_generator("lb", "logbook", 2, 0.3, 5, &ws, false).unwrap();
        let lazy = Pack::from_generator("lb", "logbook", 2, 0.3, 5, &ws, true).unwrap();
        lazy.save(dir.path()).unwrap();
        let text = std::fs::read_to_string(dir.path().join("pack.json")).unwrap();
        assert!(text.contains("\"from\""), "{text}");
        assert!(
            !text.contains("\"context\""),
            "lazy packs hold no context: {text}"
        );
        assert!(text.len() < 2000, "{}", text.len());
        let loaded = Pack::load(dir.path()).unwrap();
        assert_eq!(loaded, lazy);
        for (l, f) in loaded.tasks.iter().zip(&full.tasks) {
            assert_eq!(l.task, f.task);
            assert_eq!(l.context, f.context);
            assert_eq!(l.verify, f.verify);
            assert_eq!(l.kind, f.kind);
            assert!(l.kind.as_deref().unwrap().starts_with("logbook_"));
        }
        // A hand-written lazy task may override the generated text.
        std::fs::write(
            dir.path().join("pack.json"),
            r#"{"name":"x","description":"","kind":"k","tasks":[{"id":"t","from":{"generator":"puzzle","dial":0.1,"seed":3},"difficulty":1500}]}"#,
        )
        .unwrap();
        let p = Pack::load(dir.path()).unwrap();
        assert!(p.tasks[0].task.contains("divisible"), "{}", p.tasks[0].task);
        assert_eq!(p.tasks[0].difficulty, 1500.0);
        std::fs::write(
            dir.path().join("pack.json"),
            r#"{"name":"x","description":"","kind":"k","tasks":[{"id":"t","from":{"generator":"nope","dial":0.1,"seed":3}}]}"#,
        )
        .unwrap();
        assert!(Pack::load(dir.path()).is_err());
    }

    #[test]
    fn episodes_name_earlier_tasks_and_lineage_runs_root_first() {
        let step = |id: &str, continues: Option<&str>| PackTask {
            id: id.into(),
            kind: None,
            task: "t".into(),
            context: None,
            context_file: None,
            setup: vec![],
            workspace_from: None,
            continues: continues.map(str::to_string),
            check: None,
            verify: Verify::Human,
            difficulty: 1000.0,
            from: None,
        };
        let mut pack = Pack {
            name: "ep".into(),
            description: String::new(),
            license: String::new(),
            kind: "k".into(),
            tasks: vec![
                step("a", None),
                step("b", Some("a")),
                step("c", Some("b")),
                step("d", None),
            ],
        };
        pack.check().unwrap();
        let chain: Vec<&str> = pack
            .lineage(&pack.tasks[2])
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(chain, ["a", "b", "c"]);
        assert_eq!(pack.lineage(&pack.tasks[3]).len(), 1);
        pack.tasks.push(step("e", Some("zzz")));
        let err = pack.check().unwrap_err().to_string();
        assert!(err.contains("continues zzz"), "{err}");
        pack.tasks.pop();
        pack.tasks.push(step("a", None));
        assert!(pack.check().unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn terminal_pack_setups_and_oracles_agree_with_a_correct_solver() {
        let pack = terminal_pack();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let t = pack.tasks.iter().find(|t| t.id == "count-lines").unwrap();
            let ws = prepare_workspace(Path::new("."), t).await.unwrap();
            assert!(ws.path().join("data.txt").exists());
            let answer = kleene_core::Batch {
                schema: std::sync::Arc::new(kleene_core::Schema::new(vec![
                    kleene_core::Field::new("lines", kleene_core::DataType::Int),
                ])),
                rows: vec![vec![kleene_core::Value::Int(5)]],
            };
            assert!(t.verify.check(&answer, ws.path(), None).await.pass);
            let wrong = kleene_core::Batch {
                rows: vec![vec![kleene_core::Value::Int(4)]],
                ..answer.clone()
            };
            assert!(!t.verify.check(&wrong, ws.path(), None).await.pass);
        });
    }

    #[test]
    fn lab_import_reads_task_json_and_copies_documents() {
        let root = tempfile::tempdir().unwrap();
        let task = root
            .path()
            .join("contracting/extract-psa-key-terms/scenario-01");
        std::fs::create_dir_all(task.join("documents")).unwrap();
        std::fs::write(task.join("documents/psa.txt"), "Purchase price: 10").unwrap();
        // The shipped schema: deliverables as a map, criteria as objects.
        std::fs::write(
            task.join("task.json"),
            r#"{"title": "Extract PSA Key Terms", "instructions": "Extract the key terms of the PSA.", "work_type": "analyze", "deliverables": {"key_terms.md": "key_terms.md"}, "criteria": [{"id": "C-001", "title": "Purchase price", "deliverables": ["key_terms.md"], "match_criteria": "PASS if the purchase price is stated as 10."}]}"#,
        )
        .unwrap();
        // Two firm-knowledge tasks over one shared corpus.
        std::fs::create_dir_all(root.path().join("firm-knowledge/dms")).unwrap();
        std::fs::write(
            root.path().join("firm-knowledge/dms/deal.txt"),
            "ground lease",
        )
        .unwrap();
        for n in ["001", "002"] {
            let t = root.path().join("firm-knowledge/tasks").join(n);
            std::fs::create_dir_all(&t).unwrap();
            std::fs::write(
                t.join("task.json"),
                r#"{"id": "1", "title": "Latest deal", "instructions": "What is our latest ground-lease deal?", "docs_dir": "../../dms", "deliverables": {"response.md": "response.md"}, "criteria": [{"id": "C-001", "title": "Names the deal", "match_criteria": "Identifies the ground lease.", "deliverables": ["response.md"]}]}"#,
            )
            .unwrap();
        }
        // An older draft shape still imports.
        let legacy = root.path().join("legacy/task");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("task.json"),
            r#"{"instructions": "Do it.", "deliverables": ["out.md"], "criteria": [{"text": "It is done"}]}"#,
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        let pack = import_lab(root.path(), out.path(), None).unwrap();
        assert_eq!(pack.tasks.len(), 4);
        let t = &pack.tasks[0];
        assert_eq!(t.id, "contracting__extract-psa-key-terms__scenario-01");
        assert_eq!(t.kind.as_deref(), Some("lab_analyze"));
        assert!(t.task.contains("(expected: key_terms.md)"), "{}", t.task);
        match &t.verify {
            Verify::Judge { rubric, .. } => assert_eq!(
                rubric,
                "All of the following must hold (all-pass), judged against the files under output/:\n- C-001 Purchase price: PASS if the purchase price is stated as 10. (in key_terms.md)"
            ),
            other => panic!("{other:?}"),
        }
        let ws = t.workspace_from.as_ref().unwrap();
        assert!(out.path().join(ws).join("documents/psa.txt").exists());
        let (a, b) = (&pack.tasks[1], &pack.tasks[2]);
        assert_eq!(a.kind.as_deref(), Some("lab_task"));
        assert_eq!(a.workspace_from.as_deref(), Some("workspaces/shared-01"));
        assert_eq!(a.workspace_from, b.workspace_from);
        assert!(out
            .path()
            .join("workspaces/shared-01/documents/deal.txt")
            .exists());
        let l = &pack.tasks[3];
        assert!(l.task.contains("(expected: out.md)"));
        assert!(
            matches!(&l.verify, Verify::Judge { rubric, .. } if rubric.ends_with("- It is done"))
        );
        assert!(import_lab(out.path().join("nope").as_path(), out.path(), None).is_err());
        // A sample is deterministic, ordered, and copies only its own documents.
        let out2 = tempfile::tempdir().unwrap();
        let two = import_lab(root.path(), out2.path(), Some((2, 7))).unwrap();
        let again = import_lab(
            root.path(),
            tempfile::tempdir().unwrap().path(),
            Some((2, 7)),
        )
        .unwrap();
        assert_eq!(two.tasks.len(), 2);
        assert_eq!(
            two.tasks.iter().map(|t| &t.id).collect::<Vec<_>>(),
            again.tasks.iter().map(|t| &t.id).collect::<Vec<_>>()
        );
        let ids: Vec<&str> = pack.tasks.iter().map(|t| t.id.as_str()).collect();
        let pos: Vec<usize> = two
            .tasks
            .iter()
            .map(|t| ids.iter().position(|i| *i == t.id).unwrap())
            .collect();
        assert!(pos[0] < pos[1]);
        assert_eq!(
            pack.sampled(2, 7).tasks,
            two.tasks
                .iter()
                .map(|t| PackTask {
                    workspace_from: pack.tasks[ids.iter().position(|i| *i == t.id).unwrap()]
                        .workspace_from
                        .clone(),
                    ..t.clone()
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn sampling_is_seeded_ordered_and_without_replacement() {
        let a = sample_indices(10, 4, 1);
        assert_eq!(a, sample_indices(10, 4, 1));
        assert_eq!(a.len(), 4);
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        assert!(a.iter().all(|i| *i < 10));
        assert_ne!(a, sample_indices(10, 4, 2));
        assert_eq!(sample_indices(3, 10, 1), vec![0, 1, 2]);
        assert!(sample_indices(0, 3, 1).is_empty());
    }

    #[test]
    fn oolong_answers_parse_as_python_lists() {
        assert_eq!(oolong_answer_values("['incorrect']"), vec!["incorrect"]);
        assert_eq!(oolong_answer_values("[1542]"), vec!["1542"]);
        assert_eq!(
            oolong_answer_values("['more common than', \"it's\"]"),
            vec!["more common than", "it's"]
        );
        assert_eq!(oolong_answer_values("2023-05"), vec!["2023-05"]);
        assert!(oolong_answer_values("[]").is_empty());
    }

    #[test]
    fn redlining_rows_become_tasks_with_the_reference_in_the_oracle() {
        let rows: Vec<RedlineRow> = serde_json::from_str(
            r#"[{"instruction": "Redline this contract clause to protect the client's interests.",
                 "input": "Clause Category: governing law\nContract Type: employment_agreement\nJurisdiction: Illinois\n\nOriginal Clause:\nThis Agreement is entered into in the State of Texas and shall be interpreted according to the laws of the State of Texas.",
                 "output": "Redlined Clause:\nThis Agreement shall be governed by the laws of the State of Illinois, without regard to its conflicts of law principles.\n\nRationale:\nAligns governing law with the client's jurisdiction.\n\nSpecific Changes:\n- modification: Illinois law and a conflict-of-law exclusion.\n",
                 "metadata": {"clause_category": "governing law", "contract_type": "employment_agreement", "jurisdiction": "Illinois", "risk_reduction": "medium"}},
                {"input": "Original Clause:\nSupplier's liability is unlimited.",
                 "output": "Redlined Clause:\nSupplier's liability is capped at fees paid.\n\nRationale:\nCaps exposure.",
                 "metadata": {"clause_category": "liability"}}]"#,
        )
        .unwrap();
        let pack = redlining_pack(
            "redlining-1k",
            "UmaiTech/legal-contract-qpt5-redlining-1k",
            &rows,
        )
        .unwrap();
        pack.check().unwrap();
        assert_eq!(pack.tasks.len(), 2);
        assert_eq!(pack.tasks[0].id, "0001-governing-law");
        assert_eq!(pack.tasks[0].kind.as_deref(), Some("redline_governing_law"));
        assert!(pack.tasks[0].task.contains("under Illinois law"));
        assert!(pack.tasks[0].task.contains("State of Texas"));
        assert!(pack.tasks[0].task.contains("risk reduction: medium"));
        match &pack.tasks[0].verify {
            Verify::Redline {
                original,
                redline,
                rationale,
                changes,
            } => {
                assert!(original.starts_with("This Agreement is entered"));
                assert!(redline.ends_with("principles."));
                assert_eq!(
                    rationale,
                    "Aligns governing law with the client's jurisdiction."
                );
                assert_eq!(
                    changes,
                    &["modification: Illinois law and a conflict-of-law exclusion.".to_string()]
                );
            }
            other => panic!("{other:?}"),
        }
        // Missing sections degrade gracefully; a missing clause does not.
        match &pack.tasks[1].verify {
            Verify::Redline {
                changes, rationale, ..
            } => {
                assert!(changes.is_empty());
                assert_eq!(rationale, "Caps exposure.");
            }
            other => panic!("{other:?}"),
        }
        assert!(pack.tasks[1].difficulty < pack.tasks[0].difficulty);
        let bad = RedlineRow {
            input: "no clause here".into(),
            ..rows[1].clone()
        };
        assert!(redlining_pack("x", "y", &[bad]).is_err());
        // The pack survives a save/load round trip with its oracle intact.
        let dir = tempfile::tempdir().unwrap();
        pack.save(dir.path()).unwrap();
        assert_eq!(
            Pack::load(dir.path()).unwrap().tasks[0].verify,
            pack.tasks[0].verify
        );
    }

    #[test]
    fn oolong_pack_shares_context_files_between_questions() {
        let out = tempfile::tempdir().unwrap();
        let row = |id: i64, window: i64, task: &str, answer: &str, answer_type: &str| {
            OolongRow {
            id,
            context_len: 131072,
            dataset: "trec_coarse".into(),
            context_window_text: "The following lines contain 2 questions.\n\nYou will be asked.\n\nDate: Oct 06, 2022 || User: 1 || Instance: What is it?\nDate: Jun 11, 2025 || User: 2 || Instance: Who was it?\nRecall: the preceding lines contain 2 questions.\n\n".into(),
            question: "Which label is most common? Give your final answer in the form 'Label: answer'.".into(),
            task_group: "counting".into(),
            task: task.into(),
            answer: answer.into(),
            answer_type: answer_type.into(),
            context_window_id: window,
        }
        };
        let rows = vec![
            row(1, 7, "TASK_TYPE.MOST_FREQ", "['ENTY']", "ANSWER_TYPE.LABEL"),
            row(
                2,
                7,
                "TASK_TYPE.NUMERIC_ONE_CLASS",
                "[3]",
                "ANSWER_TYPE.NUMERIC",
            ),
            row(3, 8, "TASK_TYPE.MOST_FREQ", "['HUM']", "ANSWER_TYPE.LABEL"),
        ];
        let pack = oolong_pack("oolong-trec", &rows, out.path()).unwrap();
        assert_eq!(pack.kind, "oolong_trec_coarse");
        assert_eq!(pack.tasks.len(), 3);
        assert_eq!(pack.tasks[0].id, "trec_coarse-1");
        assert_eq!(
            pack.tasks[0].kind.as_deref(),
            Some("oolong_trec_coarse_counting")
        );
        assert_eq!(
            pack.tasks[0].context_file.as_deref(),
            Some("contexts/window-7.txt")
        );
        assert_eq!(pack.tasks[0].context_file, pack.tasks[1].context_file);
        assert_ne!(pack.tasks[0].context_file, pack.tasks[2].context_file);
        assert_eq!(
            pack.tasks[1].verify,
            Verify::Oolong {
                answer: vec!["3".into()],
                numeric: true
            }
        );
        assert!((pack.tasks[0].difficulty - 1700.0).abs() < 1e-9);
        let ctx = pack
            .context_of(out.path(), &pack.tasks[0])
            .unwrap()
            .unwrap();
        let rows_in_ctx = crate::session::paragraphs(&ctx);
        assert_eq!(rows_in_ctx.len(), 5, "{rows_in_ctx:?}");
        assert!(rows_in_ctx[2].starts_with("Date: Oct 06, 2022"));
        assert!(pack.tasks[0]
            .task
            .starts_with("Which label is most common?"));
        pack.save(out.path()).unwrap();
        assert_eq!(Pack::load(out.path()).unwrap(), pack);
    }
}
