//! Oracles: how a task's `FINAL` relation is judged. Code oracles are exact
//! and free; the judge oracle asks a separate `judge` model with a rubric
//! and a reference, never the solver's own session.

use kleene_core::{Batch, Value};
use kleene_llm::{CompletionRequest, Message, Provider, ProviderOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How an answer is checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Verify {
    /// The rendered rows must equal these (as a multiset, cells trimmed,
    /// numbers compared numerically, case-insensitive text).
    Exact {
        /// Expected rows.
        rows: Vec<Vec<String>>,
    },
    /// A 3-SAT verdict: `SAT` with a satisfying assignment, or `UNSAT` when
    /// the formula really is unsatisfiable (checked by brute force).
    Sat {
        /// Variable count.
        vars: usize,
        /// Clauses as signed variable indices.
        clauses: Vec<[i32; 3]>,
        /// Ground truth.
        satisfiable: bool,
    },
    /// A shell command run in the workspace with the answer rows as JSON on
    /// stdin; exit status 0 means pass.
    Shell {
        /// Command line.
        command: String,
    },
    /// A separate judge model grades the answer against a rubric and an
    /// optional reference; it must answer `PASS` or `FAIL`.
    Judge {
        /// What a correct answer must satisfy.
        rubric: String,
        /// A reference answer, if the proposer supplied one.
        reference: Option<String>,
    },
    /// A single number within a tolerance (finance-style answers): the first
    /// cell of the first row, commas and currency signs stripped.
    Number {
        /// Expected value.
        value: f64,
        /// Absolute tolerance.
        tolerance: f64,
    },
    /// An OOLONG answer (Bertsch et al. 2025): a label, date, user id or
    /// comparison is scored on exact match; a numeric answer scores
    /// `0.75^|expected - got|` and passes only when the score is 1. The
    /// score is reported in the detail either way, so a run's mean OOLONG
    /// score can be read back from `evals.detail`.
    Oolong {
        /// Expected values. Several means a tie (OOLONG lists every label
        /// that is equally least or most common), and any non-empty subset
        /// of them passes, one row per value.
        answer: Vec<String>,
        /// Whether the answer is a number, which earns partial credit.
        numeric: bool,
    },
    /// A contract redline (the UmaiTech `legal-contract-*-redlining`
    /// datasets): the answer's `redline` cell must be a revision of the
    /// original clause, and it is scored on how many of the reference
    /// redline's new terms it carries (`redline recall`). With a judge
    /// configured the judge decides, reading the reference redline, its
    /// rationale and the changes it lists; without one the task passes at a
    /// recall of 0.5 or more.
    Redline {
        /// The clause as the counterparty drafted it.
        original: String,
        /// The reference redline (a GPT-generated, client-protective
        /// revision), not ground truth but what the dataset scores against.
        redline: String,
        /// The reference's rationale.
        rationale: String,
        /// The specific changes the reference lists.
        changes: Vec<String>,
    },
    /// A person decides; the task waits in `needs_review`.
    Human,
}

/// The verdict on an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Passed.
    pub pass: bool,
    /// Why, for the ledger and the model.
    pub detail: String,
}

impl Verify {
    /// A short label for the board.
    pub fn label(&self) -> &'static str {
        match self {
            Verify::Exact { .. } => "exact",
            Verify::Sat { .. } => "sat",
            Verify::Shell { .. } => "shell",
            Verify::Judge { .. } => "judge",
            Verify::Number { .. } => "number",
            Verify::Oolong { .. } => "oolong",
            Verify::Redline { .. } => "redline",
            Verify::Human => "human",
        }
    }

    /// Judge an answer. `workspace` is for shell oracles; `judge` is the
    /// provider and alias for the judge oracle.
    pub async fn check(
        &self,
        answer: &Batch,
        workspace: &std::path::Path,
        judge: Option<(Arc<dyn Provider>, String)>,
    ) -> Verdict {
        match self {
            Verify::Exact { rows } => exact(rows, answer),
            Verify::Sat {
                vars,
                clauses,
                satisfiable,
            } => sat(*vars, clauses, *satisfiable, answer),
            Verify::Shell { command } => shell(command, answer, workspace).await,
            Verify::Judge { rubric, reference } => match judge {
                Some((p, alias)) => {
                    judge_call(p, &alias, rubric, reference.as_deref(), answer).await
                }
                None => Verdict {
                    pass: false,
                    detail: "no judge provider configured".into(),
                },
            },
            Verify::Number { value, tolerance } => number(*value, *tolerance, answer),
            Verify::Oolong {
                answer: want,
                numeric,
            } => oolong(want, *numeric, answer),
            Verify::Redline {
                original,
                redline,
                rationale,
                changes,
            } => redline_check(original, redline, rationale, changes, answer, judge).await,
            Verify::Human => Verdict {
                pass: false,
                detail: "awaiting human review".into(),
            },
        }
    }
}

fn number(value: f64, tolerance: f64, answer: &Batch) -> Verdict {
    let Some(cell) = answer.rows.first().and_then(|r| r.first()) else {
        return Verdict {
            pass: false,
            detail: "empty answer".into(),
        };
    };
    let text: String = cell
        .render()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    match text.parse::<f64>() {
        Ok(got) if (got - value).abs() <= tolerance => Verdict {
            pass: true,
            detail: format!("{got} within {tolerance} of {value}"),
        },
        Ok(got) => Verdict {
            pass: false,
            detail: format!("expected {value} ± {tolerance}, got {got}"),
        },
        Err(_) => Verdict {
            pass: false,
            detail: format!("expected a number, got {}", cell.render()),
        },
    }
}

/// The answer cells of a `FINAL` relation for an OOLONG question: the first
/// column, with a `Label:` or `Answer:` prefix the model may have echoed
/// from the question removed. One cell holding a comma-separated list is
/// split when several values are expected.
fn oolong_cells(answer: &Batch, expected: usize) -> Vec<String> {
    let strip = |s: &str| -> String {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        let t = ["label:", "answer:", "user:", "date:"]
            .iter()
            .find(|p| lower.starts_with(*p))
            .map(|p| &t[p.len()..])
            .unwrap_or(t);
        t.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
    };
    let mut cells: Vec<String> = answer
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|v| strip(&v.render()))
        .filter(|s| !s.is_empty())
        .collect();
    if cells.len() == 1 && expected > 1 {
        cells = cells[0]
            .split(',')
            .map(strip)
            .filter(|s| !s.is_empty())
            .collect();
    }
    cells
}

/// Score an OOLONG answer: exact match on normalised values (any of the
/// expected values when several are tied), or `0.75^|y - ŷ|` for a
/// number. Returns the score and a description.
pub fn oolong_score(want: &[String], numeric: bool, got: &[String]) -> (f64, String) {
    if numeric {
        let parse = |s: &str| -> Option<f64> {
            s.chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect::<String>()
                .parse::<f64>()
                .ok()
        };
        let Some(expected) = want.first().and_then(|s| parse(s)) else {
            return (0.0, format!("expected answer {want:?} is not a number"));
        };
        return match got.first().and_then(|s| parse(s)) {
            Some(actual) => {
                let score = 0.75f64.powf((expected - actual).abs());
                (score, format!("expected {expected}, got {actual}"))
            }
            None => (
                0.0,
                format!(
                    "expected {expected}, got {}",
                    got.first().map(String::as_str).unwrap_or("nothing")
                ),
            ),
        };
    }
    let mut w: Vec<String> = want.iter().map(|s| norm(s)).collect();
    let mut g: Vec<String> = got.iter().map(|s| norm(s)).collect();
    w.sort();
    g.sort();
    g.dedup();
    let score = if !g.is_empty() && g.iter().all(|x| w.contains(x)) {
        1.0
    } else {
        0.0
    };
    let how = if w.len() > 1 { "any of" } else { "expected" };
    (score, format!("{how} {w:?}, got {g:?}"))
}

fn oolong(want: &[String], numeric: bool, answer: &Batch) -> Verdict {
    let got = oolong_cells(answer, want.len());
    if got.is_empty() {
        return Verdict {
            pass: false,
            detail: "oolong score 0.000: empty answer".into(),
        };
    }
    let (score, why) = oolong_score(want, numeric, &got);
    Verdict {
        pass: score >= 1.0 - 1e-9,
        detail: format!("oolong score {score:.3}: {why}"),
    }
}

/// The candidate redline: the `redline` (or `redlined_clause`, `clause`,
/// `revised`) column of the first row, else its first cell.
fn redline_cell(answer: &Batch) -> Option<String> {
    let names = answer.schema.names();
    let col = names
        .iter()
        .position(|n| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "redline" | "redlined_clause" | "redlined" | "clause" | "revised" | "revision"
            )
        })
        .unwrap_or(0);
    let v = answer.rows.first()?.get(col)?;
    let text = match v {
        Value::Null => String::new(),
        other => other.render(),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Words of four letters or more, lower-cased, as a set.
fn terms(text: &str) -> std::collections::BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\''))
        .filter(|w| w.len() >= 4)
        .map(str::to_string)
        .collect()
}

/// Whitespace- and case-insensitive text, for "did it change anything".
fn squash(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// How much of the reference redline's new vocabulary (terms in the
/// reference that the original clause lacks) the candidate carries, in
/// `0..=1`. A reference that only deletes has nothing to recall, and any
/// changed candidate scores 1.
pub fn redline_recall(original: &str, reference: &str, candidate: &str) -> f64 {
    let before = terms(original);
    let added: Vec<String> = terms(reference).difference(&before).cloned().collect();
    if added.is_empty() {
        return 1.0;
    }
    let got = terms(candidate);
    added.iter().filter(|t| got.contains(*t)).count() as f64 / added.len() as f64
}

/// The rubric the judge grades a redline against.
pub fn redline_rubric(original: &str, changes: &[String]) -> String {
    let listed = if changes.is_empty() {
        String::from("- the changes the reference redline makes")
    } else {
        changes
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "The answer is a redline of this clause on behalf of the client:\n\n{original}\n\nPASS when the answer's redline is a complete, usable revision of that clause (clause text, not commentary or a diff), drafted in the client's favour, and it makes the substance of these changes, in its own words if it likes:\n{listed}\n\nFAIL when the redline leaves one of those risks unaddressed, drafts against the client, introduces a plain legal error, or is not clause text. Differences in wording, order or added protections beyond the reference do not matter."
    )
}

async fn redline_check(
    original: &str,
    reference: &str,
    rationale: &str,
    changes: &[String],
    answer: &Batch,
    judge: Option<(Arc<dyn Provider>, String)>,
) -> Verdict {
    let Some(candidate) = redline_cell(answer) else {
        return Verdict {
            pass: false,
            detail: "redline recall 0.00: empty answer".into(),
        };
    };
    if squash(&candidate) == squash(original) {
        return Verdict {
            pass: false,
            detail: "redline recall 0.00: the redline leaves the clause unchanged".into(),
        };
    }
    let recall = redline_recall(original, reference, &candidate);
    match judge {
        Some((p, alias)) => {
            let rubric = redline_rubric(original, changes);
            let reference = format!("{reference}\n\nRationale: {rationale}");
            let v = judge_call(p, &alias, &rubric, Some(&reference), answer).await;
            Verdict {
                pass: v.pass,
                detail: format!("redline recall {recall:.2}; judge: {}", v.detail),
            }
        }
        None => Verdict {
            pass: recall >= 0.5,
            detail: format!(
                "redline recall {recall:.2} of the reference's new terms (no judge configured; passes at 0.50)"
            ),
        },
    }
}

fn norm(cell: &str) -> String {
    let t = cell.trim();
    if let Ok(f) = t.parse::<f64>() {
        if f.fract() == 0.0 && f.abs() < 1e15 {
            return format!("{}", f as i64);
        }
        return format!("{f:.6}");
    }
    t.to_ascii_lowercase()
}

fn rendered_rows(answer: &Batch) -> Vec<Vec<String>> {
    answer
        .rows
        .iter()
        .map(|r| r.iter().map(|v| norm(&v.render())).collect())
        .collect()
}

fn exact(expected: &[Vec<String>], answer: &Batch) -> Verdict {
    let mut want: Vec<Vec<String>> = expected
        .iter()
        .map(|r| r.iter().map(|c| norm(c)).collect())
        .collect();
    let mut got = rendered_rows(answer);
    want.sort();
    got.sort();
    if want == got {
        Verdict {
            pass: true,
            detail: format!("{} row(s) match", got.len()),
        }
    } else {
        Verdict {
            pass: false,
            detail: format!(
                "expected {} row(s) {:?}, got {} row(s) {:?}",
                want.len(),
                preview(&want),
                got.len(),
                preview(&got)
            ),
        }
    }
}

fn preview(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter().take(4).map(|r| r.join(" | ")).collect()
}

/// Brute-force 3-SAT for small formulas; returns a satisfying assignment.
pub fn sat_solve(vars: usize, clauses: &[[i32; 3]]) -> Option<Vec<bool>> {
    if vars > 22 {
        return None;
    }
    for mask in 0u64..(1u64 << vars) {
        let assign = |v: i32| -> bool {
            let idx = (v.unsigned_abs() - 1) as usize;
            let val = mask & (1 << idx) != 0;
            if v < 0 {
                !val
            } else {
                val
            }
        };
        if clauses.iter().all(|c| c.iter().any(|&l| assign(l))) {
            return Some((0..vars).map(|i| mask & (1 << i) != 0).collect());
        }
    }
    None
}

fn sat(vars: usize, clauses: &[[i32; 3]], satisfiable: bool, answer: &Batch) -> Verdict {
    let Some(row) = answer.rows.first() else {
        return Verdict {
            pass: false,
            detail: "empty answer".into(),
        };
    };
    let names = answer.schema.names();
    let col = |name: &str| -> Option<String> {
        names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .and_then(|i| row.get(i))
            .map(|v: &Value| v.render())
    };
    let verdict = col("verdict")
        .or_else(|| row.first().map(|v| v.render()))
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    match verdict.as_str() {
        "UNSAT" => {
            if satisfiable {
                Verdict {
                    pass: false,
                    detail: "claimed UNSAT but the formula is satisfiable".into(),
                }
            } else {
                Verdict {
                    pass: true,
                    detail: "UNSAT confirmed by exhaustive search".into(),
                }
            }
        }
        "SAT" => {
            let text = col("assignment")
                .or_else(|| row.get(1).map(|v| v.render()))
                .unwrap_or_default();
            let mut truth = vec![false; vars];
            for tok in text.split(|c: char| c.is_whitespace() || c == ',') {
                let t = tok.trim().trim_start_matches('x');
                if let Ok(i) = t.parse::<usize>() {
                    if (1..=vars).contains(&i) {
                        truth[i - 1] = true;
                    }
                }
            }
            let ok = clauses.iter().all(|c| {
                c.iter().any(|&l| {
                    let v = truth[(l.unsigned_abs() - 1) as usize];
                    if l < 0 {
                        !v
                    } else {
                        v
                    }
                })
            });
            if ok {
                Verdict {
                    pass: true,
                    detail: "assignment satisfies every clause".into(),
                }
            } else {
                Verdict {
                    pass: false,
                    detail: "assignment leaves a clause false".into(),
                }
            }
        }
        other => Verdict {
            pass: false,
            detail: format!("verdict must be SAT or UNSAT, got {other:?}"),
        },
    }
}

async fn shell(command: &str, answer: &Batch, workspace: &std::path::Path) -> Verdict {
    let rows = rendered_rows(answer);
    let input = serde_json::to_string(&rows).unwrap_or_default();
    let mut child = match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Verdict {
                pass: false,
                detail: format!("oracle failed to start: {e}"),
            }
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input.as_bytes()).await;
    }
    match tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output()).await {
        Ok(Ok(out)) => Verdict {
            pass: out.status.success(),
            detail: format!(
                "exit {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
            ),
        },
        Ok(Err(e)) => Verdict {
            pass: false,
            detail: format!("oracle failed: {e}"),
        },
        Err(_) => Verdict {
            pass: false,
            detail: "oracle timed out".into(),
        },
    }
}

/// The answer as the judge reads it: every cell in full (the table
/// renderer cuts long cells, and a memo is one long cell), at most fifty
/// rows, one `column: value` line per cell.
pub fn render_for_judge(answer: &Batch) -> String {
    let names = answer.schema.names();
    let mut out = String::new();
    for (i, row) in answer.rows.iter().take(50).enumerate() {
        out.push_str(&format!("row {}:\n", i + 1));
        for (j, v) in row.iter().enumerate() {
            let name = names
                .get(j)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("col{j}"));
            out.push_str(&format!("  {name}: {}\n", v.render()));
        }
    }
    if answer.rows.len() > 50 {
        out.push_str(&format!(
            "({} more rows not shown)\n",
            answer.rows.len() - 50
        ));
    }
    if answer.rows.is_empty() {
        out.push_str("(no rows)\n");
    }
    out
}

async fn judge_call(
    provider: Arc<dyn Provider>,
    alias: &str,
    rubric: &str,
    reference: Option<&str>,
    answer: &Batch,
) -> Verdict {
    let prompt = format!(
        "You are grading an answer produced by another system. Rubric:\n{rubric}\n{}\nAnswer relation:\n{}\n\nDoes the answer satisfy the rubric? Reply with exactly PASS or FAIL on the first line, then one sentence of justification.",
        reference
            .map(|r| format!("Reference answer:\n{r}\n"))
            .unwrap_or_default(),
        render_for_judge(answer)
    );
    let req = CompletionRequest {
        alias: kleene_core::ModelAlias(alias.to_string()),
        model: alias.to_string(),
        system: "You are a strict, fair grader.".into(),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        output_schema: None,
        max_tokens: 300,
        options: ProviderOptions::default(),
    };
    match provider.complete(req).await {
        Ok(resp) => {
            let text = resp.text();
            let first = text
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_uppercase();
            Verdict {
                pass: first.starts_with("PASS") && !first.contains("FAIL"),
                detail: text.lines().take(2).collect::<Vec<_>>().join(" "),
            }
        }
        Err(e) => Verdict {
            pass: false,
            detail: format!("judge call failed: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleene_core::{DataType, Field, Schema};

    fn batch(cols: &[&str], rows: Vec<Vec<Value>>) -> Batch {
        Batch {
            schema: Arc::new(Schema::new(
                cols.iter()
                    .map(|c| Field::new(*c, DataType::Text))
                    .collect(),
            )),
            rows,
        }
    }

    #[test]
    fn exact_is_a_normalised_multiset_compare() {
        let v = Verify::Exact {
            rows: vec![
                vec!["Heron".into(), "12".into()],
                vec!["Tern".into(), "3".into()],
            ],
        };
        let b = batch(
            &["project", "total"],
            vec![
                vec![Value::from("tern"), Value::Int(3)],
                vec![Value::from(" heron "), Value::Float(12.0)],
            ],
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = std::env::temp_dir();
        assert!(rt.block_on(v.check(&b, &ws, None)).pass);
        let wrong = batch(&["p", "t"], vec![vec![Value::from("tern"), Value::Int(4)]]);
        assert!(!rt.block_on(v.check(&wrong, &ws, None)).pass);
    }

    #[test]
    fn sat_oracle_checks_assignments_and_unsat_claims() {
        // (x1 OR x2 OR x3) AND (NOT x1 OR x2 OR x3): satisfiable with x2.
        let clauses = vec![[1, 2, 3], [-1, 2, 3]];
        assert!(sat_solve(3, &clauses).is_some());
        let v = Verify::Sat {
            vars: 3,
            clauses: clauses.clone(),
            satisfiable: true,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = std::env::temp_dir();
        let good = batch(
            &["verdict", "assignment"],
            vec![vec![Value::from("SAT"), Value::from("x2")]],
        );
        assert!(rt.block_on(v.check(&good, &ws, None)).pass);
        let bad = batch(
            &["verdict", "assignment"],
            vec![vec![Value::from("SAT"), Value::from("x1")]],
        );
        assert!(!rt.block_on(v.check(&bad, &ws, None)).pass);
        let unsat_claim = batch(&["verdict"], vec![vec![Value::from("UNSAT")]]);
        assert!(!rt.block_on(v.check(&unsat_claim, &ws, None)).pass);
        // Truly unsatisfiable: all eight sign patterns over x1..x3.
        let mut all = vec![];
        for m in 0..8 {
            all.push([
                if m & 1 == 0 { 1 } else { -1 },
                if m & 2 == 0 { 2 } else { -2 },
                if m & 4 == 0 { 3 } else { -3 },
            ]);
        }
        assert!(sat_solve(3, &all).is_none());
        let v = Verify::Sat {
            vars: 3,
            clauses: all,
            satisfiable: false,
        };
        assert!(rt.block_on(v.check(&unsat_claim, &ws, None)).pass);
    }

    #[test]
    fn shell_oracle_reads_rows_on_stdin() {
        let v = Verify::Shell {
            command: "grep -q 42".into(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = std::env::temp_dir();
        let b = batch(&["n"], vec![vec![Value::Int(42)]]);
        assert!(rt.block_on(v.check(&b, &ws, None)).pass);
        let b = batch(&["n"], vec![vec![Value::Int(41)]]);
        assert!(!rt.block_on(v.check(&b, &ws, None)).pass);
    }

    #[test]
    fn oolong_oracle_scores_labels_exactly_and_numbers_with_decay() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = std::env::temp_dir();
        let label = Verify::Oolong {
            answer: vec!["incorrect".into()],
            numeric: false,
        };
        let b = batch(&["answer"], vec![vec![Value::from("Label: Incorrect")]]);
        let v = rt.block_on(label.check(&b, &ws, None));
        assert!(v.pass, "{}", v.detail);
        assert!(v.detail.starts_with("oolong score 1.000"));
        let b = batch(&["answer"], vec![vec![Value::from("correct")]]);
        assert!(!rt.block_on(label.check(&b, &ws, None)).pass);

        let num = Verify::Oolong {
            answer: vec!["1542".into()],
            numeric: true,
        };
        let b = batch(&["answer"], vec![vec![Value::Int(1542)]]);
        assert!(rt.block_on(num.check(&b, &ws, None)).pass);
        let b = batch(&["answer"], vec![vec![Value::from("Answer: 1,540")]]);
        let v = rt.block_on(num.check(&b, &ws, None));
        assert!(!v.pass);
        assert_eq!(v.detail, "oolong score 0.562: expected 1542, got 1540");
        let empty = batch(&["answer"], vec![]);
        assert!(!rt.block_on(num.check(&empty, &ws, None)).pass);

        // A tie: OOLONG lists every least-common label; naming one, or all
        // of them, is right; naming a label outside the tie is wrong.
        let tie = Verify::Oolong {
            answer: vec!["human being".into(), "location".into()],
            numeric: false,
        };
        let rows = batch(
            &["answer"],
            vec![
                vec![Value::from("location")],
                vec![Value::from("Human Being")],
            ],
        );
        assert!(rt.block_on(tie.check(&rows, &ws, None)).pass);
        let one_cell = batch(
            &["answer"],
            vec![vec![Value::from("human being, location")]],
        );
        assert!(rt.block_on(tie.check(&one_cell, &ws, None)).pass);
        let one = batch(&["answer"], vec![vec![Value::from("Label: location")]]);
        let v = rt.block_on(tie.check(&one, &ws, None));
        assert!(v.pass);
        assert_eq!(
            v.detail,
            "oolong score 1.000: any of [\"human being\", \"location\"], got [\"location\"]"
        );
        let outside = batch(
            &["answer"],
            vec![vec![Value::from("location")], vec![Value::from("entity")]],
        );
        assert!(!rt.block_on(tie.check(&outside, &ws, None)).pass);
    }

    #[test]
    fn redline_oracle_wants_a_changed_clause_and_scores_recall() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ws = std::env::temp_dir();
        let original = "This Agreement shall be governed by the laws of the State of Texas.";
        let v = Verify::Redline {
            original: original.into(),
            redline: "This Agreement shall be governed by the laws of the State of Illinois, without regard to its conflicts of law principles.".into(),
            rationale: "Aligns governing law with the client.".into(),
            changes: vec!["modification: Illinois law, conflicts exclusion".into()],
        };
        // Unchanged text (whitespace and case aside) is not a redline.
        let same = batch(
            &["redline", "rationale"],
            vec![vec![
                Value::from(original.to_uppercase()),
                Value::from("fine"),
            ]],
        );
        let r = rt.block_on(v.check(&same, &ws, None));
        assert!(!r.pass);
        assert!(r.detail.contains("unchanged"), "{}", r.detail);
        // Carrying the reference's new terms passes without a judge.
        let good = batch(
            &["rationale", "redline"],
            vec![vec![
                Value::from("client is in Illinois"),
                Value::from("This Agreement shall be governed by the laws of the State of Illinois without regard to conflicts of law principles."),
            ]],
        );
        let r = rt.block_on(v.check(&good, &ws, None));
        assert!(r.pass, "{}", r.detail);
        assert!(r.detail.starts_with("redline recall 1.00"), "{}", r.detail);
        // A change in another direction scores low and fails.
        let other = batch(
            &["redline"],
            vec![vec![Value::from(
                "This Agreement shall be governed by the laws of the State of Nevada.",
            )]],
        );
        let r = rt.block_on(v.check(&other, &ws, None));
        assert!(!r.pass);
        assert!(r.detail.starts_with("redline recall 0.00"), "{}", r.detail);
        assert!(
            !rt.block_on(v.check(&batch(&["redline"], vec![]), &ws, None))
                .pass
        );
        assert_eq!(
            redline_recall("a b", "a b", "something else entirely here"),
            1.0
        );
        assert!(redline_rubric(original, &[]).contains(original));
    }

    #[test]
    fn judge_sees_long_cells_in_full() {
        let memo = "x".repeat(400);
        let b = Batch {
            schema: Arc::new(kleene_core::Schema::new(vec![kleene_core::Field::new(
                "memo",
                kleene_core::DataType::Text,
            )])),
            rows: vec![vec![Value::Text(memo.clone())]],
        };
        let shown = render_for_judge(&b);
        assert!(shown.contains(&memo), "{shown}");
        assert!(shown.starts_with("row 1:\n  memo: "), "{shown}");
        let empty = Batch { rows: vec![], ..b };
        assert!(render_for_judge(&empty).contains("(no rows)"));
    }
}
