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

/// Import a Harvey LAB checkout: every directory with a `task.json` becomes
/// a task whose workspace is the matter folder (`documents/` copied in) and
/// whose oracle is a judge over the task's criteria, with deliverables
/// expected under `output/`. LAB's own `evaluation.run_eval` remains the
/// scorer of record; this oracle is the in-loop approximation.
pub fn import_lab(root: &Path, pack_dir: &Path) -> Result<Pack, HarnessError> {
    let mut tasks = vec![];
    let mut dirs = vec![];
    walk_for(root, "task.json", &mut dirs, 0);
    dirs.sort();
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
        let deliverables: Vec<String> = json
            .get("deliverables")
            .or_else(|| json.get("expected_deliverables"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|d| {
                        d.as_str().map(str::to_string).or_else(|| {
                            d.get("filename")
                                .and_then(|f| f.as_str())
                                .map(str::to_string)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let criteria: Vec<String> = json
            .get("criteria")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        c.as_str()
                            .map(str::to_string)
                            .or_else(|| c.get("text").and_then(|t| t.as_str()).map(str::to_string))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rubric = if criteria.is_empty() {
            format!(
                "Every requirement of the instructions is met and every deliverable ({}) was written under output/.",
                deliverables.join(", ")
            )
        } else {
            format!(
                "All of the following must hold (all-pass):\n- {}",
                criteria.join("\n- ")
            )
        };
        // Copy the matter folder next to the pack so the pack is self-contained.
        let ws_rel = format!("workspaces/{id}");
        let ws = pack_dir.join(&ws_rel);
        std::fs::create_dir_all(&ws)
            .map_err(|e| HarnessError::Config(format!("cannot create {}: {e}", ws.display())))?;
        let docs = dir.join("documents");
        if docs.is_dir() {
            copy_dir(&docs, &ws.join("documents"))?;
        }
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
        std::fs::write(
            task.join("task.json"),
            r#"{"instructions": "Extract the key terms of the PSA.", "work_type": "analyze", "deliverables": ["key_terms.md"], "criteria": [{"text": "States the purchase price"}]}"#,
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        let pack = import_lab(root.path(), out.path()).unwrap();
        assert_eq!(pack.tasks.len(), 1);
        let t = &pack.tasks[0];
        assert_eq!(t.kind.as_deref(), Some("lab_analyze"));
        assert!(t.task.contains("key_terms.md"), "{}", t.task);
        assert!(
            matches!(&t.verify, Verify::Judge { rubric, .. } if rubric.contains("purchase price"))
        );
        let ws = t.workspace_from.as_ref().unwrap();
        assert!(out.path().join(ws).join("documents/psa.txt").exists());
        assert!(import_lab(out.path().join("nope").as_path(), out.path()).is_err());
    }
}
