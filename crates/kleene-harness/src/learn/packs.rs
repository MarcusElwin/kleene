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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackTask {
    /// Stable id within the pack.
    pub id: String,
    /// Kind override.
    #[serde(default)]
    pub kind: Option<String>,
    /// Task text.
    pub task: String,
    /// Inline context (one paragraph per `ctx` row, blank-line separated).
    #[serde(default)]
    pub context: Option<String>,
    /// Context read from a file relative to the pack directory.
    #[serde(default)]
    pub context_file: Option<String>,
    /// Shell commands run in a fresh temporary workspace before the task
    /// (Terminal-Bench style setup).
    #[serde(default)]
    pub setup: Vec<String>,
    /// A directory (relative to the pack) copied into the workspace before
    /// setup, for document-heavy tasks (LAB matter folders).
    #[serde(default)]
    pub workspace_from: Option<String>,
    /// The oracle.
    pub verify: Verify,
    /// Difficulty prior in rating points.
    #[serde(default = "default_difficulty")]
    pub difficulty: f64,
}

fn default_difficulty() -> f64 {
    super::ratings::BASELINE
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
        Ok(pack)
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
    /// `seed..seed + count`, so the stream is reproducible.
    pub fn from_generator(
        name: &str,
        generator: &str,
        count: usize,
        dial: f64,
        seed: u64,
        workspace: &Path,
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
                kind: None,
                task: g.task,
                context: g.context,
                context_file: None,
                setup: vec![],
                workspace_from: None,
                verify: g.verify,
                difficulty: g.difficulty,
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
    fn create() -> Result<Self, HarnessError> {
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
    Ok(dir)
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
        verify: Verify::Shell {
            command: check.into(),
        },
        difficulty: 1000.0,
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
            verify: Verify::Judge {
                rubric,
                reference: None,
            },
            difficulty: 1400.0,
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
        let a = Pack::from_generator("corp", "corpus", 3, 0.4, 11, &ws).unwrap();
        let b = Pack::from_generator("corp", "corpus", 3, 0.4, 11, &ws).unwrap();
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
