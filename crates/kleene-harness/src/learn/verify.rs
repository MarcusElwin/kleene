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
        answer.render_table(50)
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
                pass: first.starts_with("PASS"),
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
}
