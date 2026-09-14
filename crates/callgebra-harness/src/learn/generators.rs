//! Task generators with code oracles. Every generator is a pure function of
//! its hardness dial and a seed, so a task can be regenerated and its
//! reference answer recomputed; the oracle is the [`Verify`] spec attached
//! to the task, never the model that solves it.

use super::verify::Verify;
use std::collections::{HashMap, VecDeque};

/// A generated task before it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedTask {
    /// Generator name.
    pub generator: String,
    /// Task kind, for playbook lookup (`sat3`, `graph_path`, ...).
    pub kind: String,
    /// The task text the solver sees.
    pub task: String,
    /// Context loaded as `ctx(ordinal, text)`, if any.
    pub context: Option<String>,
    /// How the answer is checked.
    pub verify: Verify,
    /// The dial the task was generated at.
    pub dial: f64,
    /// Difficulty prior in rating points (1000 = even odds against a
    /// baseline solver).
    pub difficulty: f64,
}

/// Generators the harness knows.
pub const GENERATORS: &[&str] = &[
    "sat3",
    "graph",
    "puzzle",
    "corpus",
    "repo",
    "statements",
    "contracts",
];

/// A small deterministic PRNG (xorshift64*), so tasks are reproducible.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeded generator.
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Generate one task. `dial` is the generator's hardness knob in `[0, 1]`;
/// `workspace` is needed by `repo`.
pub fn generate(
    generator: &str,
    dial: f64,
    seed: u64,
    workspace: &std::path::Path,
) -> Option<GeneratedTask> {
    let dial = dial.clamp(0.0, 1.0);
    let mut rng = Rng::new(seed);
    Some(match generator {
        "sat3" => sat3(dial, &mut rng),
        "graph" => graph(dial, &mut rng),
        "puzzle" => puzzle(dial, &mut rng),
        "corpus" => corpus(dial, &mut rng),
        "repo" => repo(dial, &mut rng, workspace)?,
        "statements" => statements(dial, &mut rng),
        "contracts" => contracts(dial, &mut rng),
        _ => return None,
    })
}

/// Finance: a synthetic income statement with planted figures across years
/// and a numerical question (growth, ratio or sum). The dial adds years,
/// line items and distractor notes.
fn statements(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let years = 2 + (dial * 3.0).round() as usize;
    let items = [
        "Revenue",
        "Cost of revenue",
        "Operating expenses",
        "Interest expense",
        "Income tax",
        "Depreciation",
    ];
    let n_items = 3 + (dial * 3.0).round() as usize;
    let mut table: Vec<(String, Vec<i64>)> = vec![];
    for item in items.iter().take(n_items) {
        let base = 500 + rng.below(9000) as i64;
        let vals: Vec<i64> = (0..years)
            .map(|y| base + (y as i64) * (rng.below(400) as i64 - 100))
            .collect();
        table.push((item.to_string(), vals));
    }
    let mut lines = vec![format!(
        "Consolidated statement of operations (in thousands). Fiscal years: {}.",
        (0..years)
            .map(|y| (2020 + y).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )];
    for (item, vals) in &table {
        lines.push(format!(
            "{item}: {}",
            vals.iter()
                .enumerate()
                .map(|(y, v)| format!("FY{}: {}", 2020 + y, v))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let distractors = (dial * 6.0).round() as usize;
    for i in 0..distractors {
        lines.push(format!(
            "Note {}: figures are unaudited; segment {} reclassified in FY{}.",
            i + 1,
            i % 3 + 1,
            2020 + rng.below(years as u64) as usize
        ));
    }
    let (question, value, tolerance) = match rng.below(3) {
        0 => {
            let (item, vals) = &table[0];
            let a = vals[years - 2] as f64;
            let b = vals[years - 1] as f64;
            let growth = (b - a) / a * 100.0;
            (
                format!(
                    "By what percentage did {item} change from FY{} to FY{}? Answer with FINAL over one row with column pct (a number, one decimal is fine).",
                    2020 + years - 2,
                    2020 + years - 1
                ),
                growth,
                0.15,
            )
        }
        1 => {
            let (a_item, a_vals) = &table[0];
            let (b_item, b_vals) = &table[1];
            let ratio = a_vals[years - 1] as f64 / b_vals[years - 1] as f64;
            (
                format!(
                    "What is the ratio of {a_item} to {b_item} in FY{}? Answer with FINAL over one row with column ratio (two decimals).",
                    2020 + years - 1
                ),
                ratio,
                0.015,
            )
        }
        _ => {
            let total: i64 = table.iter().map(|(_, v)| v[years - 1]).sum();
            (
                format!(
                    "What is the sum of every line item in FY{}? Answer with FINAL over one row with column total (in thousands).",
                    2020 + years - 1
                ),
                total as f64,
                0.5,
            )
        }
    };
    GeneratedTask {
        generator: "statements".into(),
        kind: "finance_statement".into(),
        task: format!("ctx holds a financial statement, one line per row. {question}"),
        context: Some(lines.join("\n\n")),
        verify: Verify::Number { value, tolerance },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

/// Legal: a synthetic contract with planted clause categories among
/// boilerplate; the task is to list which categories are present. The dial
/// adds length, paraphrase and near-miss distractors.
fn contracts(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let categories: [(&str, [&str; 2]); 6] = [
        (
            "non-compete",
            [
                "The Supplier shall not engage in any business competing with the Customer for a period of two years.",
                "During the Restricted Period the Supplier agrees not to compete, directly or indirectly, with the Customer.",
            ],
        ),
        (
            "termination for convenience",
            [
                "Either party may terminate this Agreement for any reason upon ninety days' written notice.",
                "This Agreement may be terminated by either party without cause on ninety days' notice.",
            ],
        ),
        (
            "cap on liability",
            [
                "In no event shall either party's aggregate liability exceed the fees paid in the preceding twelve months.",
                "Each party's total liability under this Agreement is limited to the amounts paid in the prior year.",
            ],
        ),
        (
            "governing law",
            [
                "This Agreement shall be governed by the laws of the State of Delaware.",
                "The laws of Delaware govern this Agreement and any dispute arising from it.",
            ],
        ),
        (
            "audit rights",
            [
                "The Customer may audit the Supplier's relevant records once per year on reasonable notice.",
                "Upon reasonable notice the Customer shall have the right to inspect the Supplier's records annually.",
            ],
        ),
        (
            "exclusivity",
            [
                "The Customer shall purchase the Products exclusively from the Supplier during the Term.",
                "During the Term the Supplier is the Customer's sole source for the Products.",
            ],
        ),
    ];
    let boilerplate = [
        "The parties agree that headings are for convenience only.",
        "Notices shall be delivered in writing to the addresses set out in Schedule 1.",
        "This Agreement constitutes the entire agreement between the parties.",
        "If any provision is held invalid, the remainder shall continue in effect.",
        "Each party shall bear its own costs in connection with this Agreement.",
        "The Supplier shall deliver the Products in accordance with Schedule 2.",
        "Nothing in this Agreement creates a partnership or agency between the parties.",
    ];
    let near_misses = [
        "The Supplier may compete freely with the Customer after the Term ends.",
        "This Agreement may not be terminated except for material breach.",
        "Liability for gross negligence is unlimited.",
    ];
    let n_present = 1 + rng.below(4) as usize;
    let mut present: Vec<usize> = vec![];
    while present.len() < n_present {
        let c = rng.below(categories.len() as u64) as usize;
        if !present.contains(&c) {
            present.push(c);
        }
    }
    let mut clauses: Vec<String> = vec![];
    let length = 6 + (dial * 18.0).round() as usize;
    for i in 0..length {
        clauses.push(format!(
            "{}. {}",
            i + 1,
            boilerplate[rng.below(boilerplate.len() as u64) as usize]
        ));
    }
    for &c in &present {
        let variant = if rng.unit() < dial { 1 } else { 0 };
        let pos = rng.below(clauses.len() as u64) as usize;
        clauses.insert(pos, format!("{}a. {}", pos + 1, categories[c].1[variant]));
    }
    if dial > 0.4 {
        let pos = rng.below(clauses.len() as u64) as usize;
        clauses.insert(
            pos,
            format!(
                "{}b. {}",
                pos + 1,
                near_misses[rng.below(near_misses.len() as u64) as usize]
            ),
        );
    }
    let mut rows: Vec<Vec<String>> = present
        .iter()
        .map(|&c| vec![categories[c].0.to_string()])
        .collect();
    rows.sort();
    GeneratedTask {
        generator: "contracts".into(),
        kind: "legal_clause_categories".into(),
        task: format!(
            "ctx holds a contract, one clause per row. Which of these clause categories does it contain: {}? \
Answer with FINAL over one row per present category with column category (exactly the category name as written here).",
            categories
                .iter()
                .map(|(n, _)| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        context: Some(clauses.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

fn difficulty_prior(dial: f64) -> f64 {
    // 0 -> 800 (easy), 1 -> 1600 (hard).
    800.0 + 800.0 * dial
}

/// Random 3-SAT at a clause/variable ratio near the phase transition; the
/// dial sets the variable count (4..=12) and the ratio (3.0..=4.6).
fn sat3(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let n = 4 + (dial * 8.0).round() as usize;
    let ratio = 3.0 + 1.6 * dial;
    let m = ((n as f64) * ratio).round() as usize;
    let mut clauses: Vec<[i32; 3]> = vec![];
    for _ in 0..m {
        let mut lits = [0i32; 3];
        let mut used = vec![];
        for l in lits.iter_mut() {
            let mut v;
            loop {
                v = 1 + rng.below(n as u64) as i32;
                if !used.contains(&v) {
                    break;
                }
            }
            used.push(v);
            *l = if rng.below(2) == 0 { v } else { -v };
        }
        clauses.push(lits);
    }
    let rendered: Vec<String> = clauses
        .iter()
        .map(|c| format!("({} OR {} OR {})", lit(c[0]), lit(c[1]), lit(c[2])))
        .collect();
    let satisfiable = super::verify::sat_solve(n, &clauses).is_some();
    GeneratedTask {
        generator: "sat3".into(),
        kind: "sat3".into(),
        task: format!(
            "Decide whether this 3-SAT formula over variables x1..x{n} is satisfiable. Clauses (one per row of ctx too): {}. \
Answer with FINAL over one row: column verdict = 'SAT' or 'UNSAT', and if SAT a column assignment listing the true variables separated by spaces (e.g. 'x1 x3').",
            rendered.join(" AND ")
        ),
        context: Some(rendered.join("\n\n")),
        verify: Verify::Sat {
            vars: n,
            clauses: clauses.clone(),
            satisfiable,
        },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

fn lit(l: i32) -> String {
    if l < 0 {
        format!("NOT x{}", -l)
    } else {
        format!("x{l}")
    }
}

/// Shortest path in a random directed graph; the dial sets the node count
/// and edge density.
fn graph(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let n = 5 + (dial * 15.0).round() as usize;
    let density = 0.15 + 0.2 * dial;
    let mut edges: Vec<(usize, usize)> = vec![];
    for a in 0..n {
        for b in 0..n {
            if a != b && rng.unit() < density {
                edges.push((a, b));
            }
        }
    }
    // Guarantee a spine so the graph is connected enough to be interesting.
    for a in 0..n.saturating_sub(1) {
        if rng.unit() < 0.7 && !edges.contains(&(a, a + 1)) {
            edges.push((a, a + 1));
        }
    }
    let from = 0;
    let to = n - 1;
    let dist = bfs(n, &edges, from, to);
    let lines: Vec<String> = edges.iter().map(|(a, b)| format!("n{a} -> n{b}")).collect();
    GeneratedTask {
        generator: "graph".into(),
        kind: "graph_path".into(),
        task: format!(
            "A directed graph is given in ctx, one edge per row as 'nA -> nB'. \
What is the length (number of edges) of the shortest path from n{from} to n{to}? \
Answer with FINAL over one row with column hops (an integer), or hops = -1 if there is no path."
        ),
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact {
            rows: vec![vec![dist.map(|d| d.to_string()).unwrap_or("-1".into())]],
        },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

fn bfs(n: usize, edges: &[(usize, usize)], from: usize, to: usize) -> Option<usize> {
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (a, b) in edges {
        adj.entry(*a).or_default().push(*b);
    }
    let mut dist = vec![usize::MAX; n];
    dist[from] = 0;
    let mut q = VecDeque::from([from]);
    while let Some(u) = q.pop_front() {
        if u == to {
            return Some(dist[u]);
        }
        for &v in adj.get(&u).map(|v| v.as_slice()).unwrap_or(&[]) {
            if dist[v] == usize::MAX {
                dist[v] = dist[u] + 1;
                q.push_back(v);
            }
        }
    }
    None
}

/// A procedural arithmetic puzzle over a list of numbers in ctx: sum the
/// numbers divisible by k. The dial sets the list length and the divisor.
fn puzzle(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let len = 8 + (dial * 40.0).round() as usize;
    let k = 2 + rng.below(2 + (dial * 5.0).round() as u64) as i64;
    let nums: Vec<i64> = (0..len).map(|_| 1 + rng.below(200) as i64).collect();
    let total: i64 = nums.iter().filter(|x| *x % k == 0).sum();
    GeneratedTask {
        generator: "puzzle".into(),
        kind: "puzzle_divisible_sum".into(),
        task: format!(
            "ctx holds {len} integers, one per row. Answer with FINAL over one row with column total = the sum of the numbers divisible by {k} (0 if none)."
        ),
        context: Some(
            nums.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        verify: Verify::Exact {
            rows: vec![vec![total.to_string()]],
        },
        dial,
        difficulty: difficulty_prior(dial * 0.6),
    }
}

/// OOLONG-like: dated meeting notes about projects, with a per-project
/// aggregate to compute. The dial sets the note count and distractor rate.
fn corpus(dial: f64, rng: &mut Rng) -> GeneratedTask {
    let projects = ["Heron", "Kestrel", "Osprey", "Plover", "Sandpiper", "Tern"];
    let people = ["Amara", "Bao", "Chidi", "Dana", "Eitan", "Farah"];
    let fillers = [
        "The coffee machine is broken again.",
        "Parking closes at 22:00.",
        "Design review moved rooms.",
        "All-hands cancelled next week.",
    ];
    let n_projects = 2 + (dial * 4.0).round() as usize;
    let notes = 12 + (dial * 60.0).round() as usize;
    let mut hours: HashMap<&str, i64> = HashMap::new();
    let mut lines = vec![];
    for i in 0..notes {
        let p = projects[rng.below(n_projects as u64) as usize];
        let who = people[rng.below(people.len() as u64) as usize];
        let h = 1 + rng.below(9) as i64;
        *hours.entry(p).or_default() += h;
        let filler = if rng.unit() < 0.3 + 0.5 * dial {
            format!(" {}", fillers[rng.below(fillers.len() as u64) as usize])
        } else {
            String::new()
        };
        lines.push(format!(
            "Meeting notes 2026-03-{:02}. Project {p}: {who} logged {h} hours.{filler}",
            1 + i % 28
        ));
    }
    let mut rows: Vec<Vec<String>> = hours
        .iter()
        .map(|(p, h)| vec![p.to_string(), h.to_string()])
        .collect();
    rows.sort();
    GeneratedTask {
        generator: "corpus".into(),
        kind: "corpus_hours_by_project".into(),
        task: "ctx holds meeting notes, one per row, each mentioning a project and hours logged. \
Answer with FINAL over one row per project: columns project (the project name) and total_hours (an integer)."
            .into(),
        context: Some(lines.join("\n\n")),
        verify: Verify::Exact { rows },
        dial,
        difficulty: difficulty_prior(dial),
    }
}

/// A question about the workspace checkout answered by walking it: how many
/// files with a given extension live under a directory. The dial picks
/// deeper directories.
fn repo(dial: f64, rng: &mut Rng, workspace: &std::path::Path) -> Option<GeneratedTask> {
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    collect_dirs(
        workspace,
        workspace,
        0,
        1 + (dial * 3.0).round() as usize,
        &mut dirs,
    );
    if dirs.is_empty() {
        return None;
    }
    let dir = dirs[rng.below(dirs.len() as u64) as usize].clone();
    let exts = ["rs", "md", "toml", "sql", "txt", "yml"];
    let ext = exts[rng.below(exts.len() as u64) as usize];
    let count = count_files(&workspace.join(&dir), ext);
    let rel = dir.display().to_string();
    let rel = if rel.is_empty() { ".".to_string() } else { rel };
    Some(GeneratedTask {
        generator: "repo".into(),
        kind: "repo_count_files".into(),
        task: format!(
            "In this workspace, how many files with the extension .{ext} are there under the directory '{rel}' (recursively)? \
Use the files tool. Answer with FINAL over one row with column count (an integer)."
        ),
        context: None,
        verify: Verify::Exact {
            rows: vec![vec![count.to_string()]],
        },
        dial,
        difficulty: difficulty_prior(dial),
    })
}

fn collect_dirs(
    root: &std::path::Path,
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<std::path::PathBuf>,
) {
    if depth > max_depth {
        return;
    }
    if let Ok(rel) = dir.strip_prefix(root) {
        out.push(rel.to_path_buf());
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if e.path().is_dir() {
            collect_dirs(root, &e.path(), depth + 1, max_depth, out);
        }
    }
}

fn count_files(dir: &std::path::Path, ext: &str) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            n += count_files(&p, ext);
        } else if p.extension().is_some_and(|x| x == ext) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic_and_self_consistent() {
        let ws = std::env::temp_dir();
        for g in GENERATORS {
            if *g == "repo" {
                continue;
            }
            let a = generate(g, 0.5, 7, &ws).unwrap();
            let b = generate(g, 0.5, 7, &ws).unwrap();
            assert_eq!(a, b, "{g} must be deterministic");
            let c = generate(g, 0.5, 8, &ws).unwrap();
            assert_ne!(
                a.task.clone() + &a.context.clone().unwrap_or_default(),
                c.task + &c.context.unwrap_or_default(),
                "{g} must vary with the seed"
            );
            assert!(a.difficulty >= 800.0 && a.difficulty <= 1600.0);
        }
    }

    #[test]
    fn dial_makes_tasks_bigger() {
        let ws = std::env::temp_dir();
        let easy = generate("puzzle", 0.0, 1, &ws).unwrap();
        let hard = generate("puzzle", 1.0, 1, &ws).unwrap();
        assert!(hard.context.unwrap().lines().count() > easy.context.unwrap().lines().count());
        let easy = generate("sat3", 0.0, 1, &ws).unwrap();
        let hard = generate("sat3", 1.0, 1, &ws).unwrap();
        assert!(hard.context.unwrap().lines().count() > easy.context.unwrap().lines().count());
    }

    #[test]
    fn bfs_finds_shortest_paths() {
        assert_eq!(bfs(4, &[(0, 1), (1, 2), (2, 3), (0, 3)], 0, 3), Some(1));
        assert_eq!(bfs(3, &[(0, 1)], 0, 2), None);
    }
}
