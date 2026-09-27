//! The learned function ledger: versions of prompt-defined functions, refined
//! by the model from failed attempts and adopted only through the same
//! replay gate the playbook uses. Every session starts with the adopted
//! versions defined (see [`crate::HarnessConfig::functions`]), so a refined
//! `verify` is what the next task calls.

use super::{int_at, now, s, text_at, Learn};
use crate::HarnessError;
use kleene_core::Batch;
use kleene_store::DuckDbStore;
use serde::{Deserialize, Serialize};

/// Tables of the ledger; created by [`adopted_definitions`] and by
/// [`Learn::new`](super::Learn::new).
pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS functions (version INTEGER PRIMARY KEY, name VARCHAR, definition VARCHAR, kind VARCHAR, adopted BOOLEAN, learned_from VARCHAR, eval_note VARCHAR, created_at TIMESTAMP)",
    "CREATE TABLE IF NOT EXISTS function_evals (run_at TIMESTAMP, name VARCHAR, kind VARCHAR, candidate INTEGER, baseline_solved INTEGER, candidate_solved INTEGER, total INTEGER, baseline_dollars DOUBLE, candidate_dollars DOUBLE, adopted BOOLEAN)",
];

/// One version in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionVersion {
    /// Ledger version.
    pub version: i64,
    /// Function name (lower case).
    pub name: String,
    /// The full `CREATE OR REPLACE FUNCTION` statement.
    pub definition: String,
    /// Task kind the version was refined for (empty for a hand-added one).
    pub kind: String,
    /// Whether sessions define it.
    pub adopted: bool,
    /// Why it was adopted or rejected.
    pub eval_note: String,
}

/// What `learn refine` did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefineReport {
    /// The function refined.
    pub name: String,
    /// The version the candidate replaces.
    pub baseline: i64,
    /// The candidate version.
    pub candidate: i64,
    /// The candidate's definition.
    pub definition: String,
    /// Whether the replay gate adopted it.
    pub adopted: bool,
    /// The gate's note.
    pub note: String,
}

/// The adopted definitions, one per name, newest version first by name.
/// Creates the ledger tables when they do not exist, so the harness can call
/// this on any store.
pub async fn adopted_definitions(store: &DuckDbStore) -> Result<Vec<String>, HarnessError> {
    for ddl in DDL {
        store.execute(ddl).await?;
    }
    let b = store
        .query("SELECT definition FROM functions WHERE adopted ORDER BY name, version")
        .await?;
    Ok((0..b.rows.len()).map(|i| text_at(&b, i, 0)).collect())
}

/// The name of the function a `CREATE FUNCTION` statement defines, after
/// validating the statement.
fn defined_name(definition: &str) -> Result<String, HarnessError> {
    let stmt = kleene_sql::plan_create_function(definition)
        .map_err(|e| HarnessError::Config(format!("not a valid CREATE FUNCTION: {e}")))?;
    match stmt.kind {
        kleene_sql::StatementKind::CreateFunction { name, .. } => Ok(name.to_ascii_lowercase()),
        _ => Err(HarnessError::Config("not a CREATE FUNCTION".into())),
    }
}

/// `CREATE FUNCTION` statements with prompt bodies in a script, as written.
pub(crate) fn prompt_definitions(sql: &str) -> Vec<(String, String)> {
    crate::repl::split_statements(sql)
        .into_iter()
        .filter_map(|stmt| {
            let planned = kleene_sql::plan_create_function(&stmt).ok()?;
            match planned.kind {
                kleene_sql::StatementKind::CreateFunction {
                    name,
                    body: kleene_sql::FunctionBody::Prompt { .. },
                    ..
                } => Some((name.to_ascii_lowercase(), stmt)),
                _ => None,
            }
        })
        .collect()
}

/// The reply's `CREATE ... FUNCTION` statement, with any code fence removed.
fn extract_definition(reply: &str) -> Option<String> {
    let text = reply.trim();
    let body = if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.split_once('\n').map(|(_, r)| r).unwrap_or(after);
        after.split("```").next().unwrap_or(after)
    } else {
        text
    };
    let upper = body.to_ascii_uppercase();
    let start = upper.find("CREATE")?;
    let stmt = body[start..].trim().trim_end_matches(';').trim();
    (!stmt.is_empty()).then(|| stmt.to_string())
}

impl Learn {
    /// Add a definition to the ledger as the adopted version of its name
    /// (the baseline a later `refine` improves on). Any earlier adopted
    /// version of the same name is withdrawn.
    pub async fn function_add(
        &self,
        definition: &str,
        learned_from: &str,
    ) -> Result<i64, HarnessError> {
        let name = defined_name(definition)?;
        let version = self
            .insert_function(&name, definition, "", true, learned_from, "added by hand")
            .await?;
        self.store
            .execute(&format!(
                "UPDATE functions SET adopted = false WHERE name = {} AND version <> {version} AND adopted",
                s(&name)
            ))
            .await?;
        Ok(version)
    }

    async fn insert_function(
        &self,
        name: &str,
        definition: &str,
        kind: &str,
        adopted: bool,
        learned_from: &str,
        note: &str,
    ) -> Result<i64, HarnessError> {
        let b = self
            .store()
            .query("SELECT COALESCE(MAX(version), 0) + 1 FROM functions")
            .await?;
        let version = int_at(&b, 0, 0);
        self.store()
            .execute(&format!(
                "INSERT INTO functions VALUES ({version}, {}, {}, {}, {adopted}, {}, {}, {})",
                s(name),
                s(definition),
                s(kind),
                s(learned_from),
                s(note),
                now()
            ))
            .await?;
        Ok(version)
    }

    /// Record the prompt functions a solved run defined, as adopted baseline
    /// versions of names the ledger does not know yet. Refinement starts
    /// from what the model wrote.
    pub(crate) async fn seed_functions(
        &self,
        sql: &str,
        learned_from: &str,
    ) -> Result<Vec<i64>, HarnessError> {
        let mut out = vec![];
        for (name, definition) in prompt_definitions(sql) {
            let known = self
                .store()
                .query(&format!(
                    "SELECT COUNT(*) FROM functions WHERE name = {}",
                    s(&name)
                ))
                .await?;
            if int_at(&known, 0, 0) > 0 {
                continue;
            }
            out.push(
                self.insert_function(
                    &name,
                    &definition,
                    "",
                    true,
                    learned_from,
                    "seeded from a solved run",
                )
                .await?,
            );
        }
        Ok(out)
    }

    /// The ledger.
    pub async fn functions(&self) -> Result<Batch, HarnessError> {
        Ok(self
            .store()
            .query("SELECT version, name, kind, adopted, eval_note, definition FROM functions ORDER BY version")
            .await?)
    }

    /// One version.
    pub async fn function_version(&self, version: i64) -> Result<FunctionVersion, HarnessError> {
        let b = self
            .store()
            .query(&format!(
                "SELECT name, definition, kind, adopted, eval_note FROM functions WHERE version = {version}"
            ))
            .await?;
        if b.rows.is_empty() {
            return Err(HarnessError::Config(format!(
                "no function version {version}"
            )));
        }
        Ok(FunctionVersion {
            version,
            name: text_at(&b, 0, 0),
            definition: text_at(&b, 0, 1),
            kind: text_at(&b, 0, 2),
            adopted: text_at(&b, 0, 3) == "true",
            eval_note: text_at(&b, 0, 4),
        })
    }

    /// The adopted version of `name`, if any.
    pub async fn adopted_function(
        &self,
        name: &str,
    ) -> Result<Option<FunctionVersion>, HarnessError> {
        let b = self
            .store()
            .query(&format!(
                "SELECT version FROM functions WHERE name = {} AND adopted ORDER BY version DESC LIMIT 1",
                s(&name.to_ascii_lowercase())
            ))
            .await?;
        if b.rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.function_version(int_at(&b, 0, 0)).await?))
    }

    /// The adopted definitions with `name` replaced by `definition`: what a
    /// candidate arm of the replay gate defines.
    async fn definitions_with(
        &self,
        name: &str,
        definition: &str,
    ) -> Result<Vec<String>, HarnessError> {
        let b = self
            .store()
            .query("SELECT name, definition FROM functions WHERE adopted ORDER BY name, version")
            .await?;
        let mut out: Vec<String> = (0..b.rows.len())
            .filter(|&i| text_at(&b, i, 0) != name)
            .map(|i| text_at(&b, i, 1))
            .collect();
        out.push(definition.to_string());
        Ok(out)
    }

    /// Ask the model for a better prompt for `name`, from the adopted
    /// definition and the recent failed attempts (of `kind`, or any), then
    /// put the candidate through the replay gate. Returns what happened;
    /// the candidate is in the ledger either way.
    pub async fn refine(
        &self,
        name: &str,
        kind: Option<&str>,
    ) -> Result<RefineReport, HarnessError> {
        let name = name.to_ascii_lowercase();
        let Some(current) = self.adopted_function(&name).await? else {
            return Err(HarnessError::Config(format!(
                "no adopted definition of {name}; add one with `learn function add` or solve a task that defines it"
            )));
        };
        let Some(provider) = &self.harness_cfg().provider else {
            return Err(HarnessError::Config("no provider to refine with".into()));
        };
        let kind_filter = kind
            .map(|k| format!("AND t.kind = {}", s(k)))
            .unwrap_or_default();
        let failures = self
            .store()
            .query(&format!(
                "SELECT t.task, a.detail FROM attempts a JOIN tasks t ON t.id = a.task WHERE NOT a.solved {kind_filter} ORDER BY a.recorded_at DESC LIMIT 5"
            ))
            .await?;
        let mut evidence = String::new();
        for i in 0..failures.rows.len() {
            let task: String = text_at(&failures, i, 0).chars().take(400).collect();
            let detail: String = text_at(&failures, i, 1).chars().take(300).collect();
            evidence.push_str(&format!("- task: {task}\n  outcome: {detail}\n"));
        }
        if evidence.is_empty() {
            evidence.push_str("(no failed attempts recorded yet)\n");
        }
        let prompt = format!(
            "This prompt-defined SQL function is used by an agent that answers tasks with SQL over model calls:\n\n{}\n\nRecent failed attempts{}:\n{evidence}\nRewrite the prompt template so the function answers more reliably: be precise about the decision, the answer format and edge cases, keep the same name, arguments, return type and options (MODEL, BATCH, PROXY). Reply with the complete CREATE OR REPLACE FUNCTION statement and nothing else.",
            current.definition,
            kind.map(|k| format!(" on tasks of kind {k}")).unwrap_or_default()
        );
        let req = kleene_llm::CompletionRequest {
            alias: kleene_core::ModelAlias(self.cfg().judge_alias.clone()),
            model: self.cfg().judge_alias.clone(),
            system: "You refine prompt templates for SQL functions. Reply with SQL only.".into(),
            messages: vec![kleene_llm::Message::user(prompt)],
            tools: vec![],
            output_schema: None,
            max_tokens: 1200,
            options: Default::default(),
        };
        let resp = provider
            .complete(req)
            .await
            .map_err(|e| HarnessError::Config(format!("refine call failed: {e}")))?;
        let reply = resp.text();
        let definition = extract_definition(&reply).ok_or_else(|| {
            HarnessError::Config(format!("refine did not return a CREATE FUNCTION: {reply}"))
        })?;
        let defined = defined_name(&definition)?;
        if defined != name {
            return Err(HarnessError::Config(format!(
                "refine renamed {name} to {defined}; rejected"
            )));
        }
        let definition = if definition
            .to_ascii_uppercase()
            .starts_with("CREATE OR REPLACE")
        {
            definition
        } else {
            format!("CREATE OR REPLACE{}", &definition["CREATE".len()..])
        };
        let candidate = self
            .insert_function(&name, &definition, kind.unwrap_or(""), false, "refine", "")
            .await?;
        let (adopted, note) = self.gate_function(&current, candidate, kind).await?;
        Ok(RefineReport {
            name,
            baseline: current.version,
            candidate,
            definition,
            adopted,
            note,
        })
    }

    /// Replay gate for a function candidate: the same sample as the playbook
    /// gate, run once with the adopted definitions and once with the
    /// candidate in place of its baseline. Adopted when it solves at least
    /// as many tasks at no more cost; the eval is recorded either way.
    async fn gate_function(
        &self,
        baseline: &FunctionVersion,
        candidate: i64,
        kind: Option<&str>,
    ) -> Result<(bool, String), HarnessError> {
        let cand = self.function_version(candidate).await?;
        let kind_filter = kind
            .map(|k| format!("AND kind = {}", s(k)))
            .unwrap_or_default();
        let sample = if self.cfg().replay_sample == 0 {
            Batch::empty(std::sync::Arc::new(kleene_core::Schema::new(vec![])))
        } else {
            self.store()
                .query(&format!(
                    "SELECT id FROM tasks WHERE status IN ('solved', 'failed') {kind_filter} ORDER BY updated_at DESC LIMIT {}",
                    self.cfg().replay_sample
                ))
                .await?
        };
        let ids: Vec<String> = (0..sample.rows.len())
            .map(|i| text_at(&sample, i, 0))
            .collect();
        let with_candidate = self.definitions_with(&cand.name, &cand.definition).await?;
        let (mut b_solved, mut b_cost, mut c_solved, mut c_cost) = (0, 0.0, 0, 0.0);
        for id in &ids {
            let Some(task) = self.task(id).await? else {
                continue;
            };
            let playbook = self.playbook_for(&task.kind).await?;
            let br = self
                .run_task_with(&task, playbook.clone(), true, None, None)
                .await?;
            if self
                .judge_in(&task, &br, &self.harness_cfg().workspace)
                .await
                .pass
            {
                b_solved += 1;
            }
            b_cost += br.root.usage.dollars;
            let cr = self
                .run_task_with(&task, playbook, true, None, Some(with_candidate.clone()))
                .await?;
            if self
                .judge_in(&task, &cr, &self.harness_cfg().workspace)
                .await
                .pass
            {
                c_solved += 1;
            }
            c_cost += cr.root.usage.dollars;
        }
        let adopted = ids.is_empty() || (c_solved >= b_solved && c_cost <= b_cost + 1e-9);
        self.store()
            .execute(&format!(
                "INSERT INTO function_evals VALUES ({}, {}, {}, {candidate}, {b_solved}, {c_solved}, {}, {b_cost}, {c_cost}, {adopted})",
                now(),
                s(&cand.name),
                s(kind.unwrap_or("")),
                ids.len()
            ))
            .await?;
        let note = if ids.is_empty() {
            "adopted without replay (no tasks to replay, or replay_sample = 0)".to_string()
        } else {
            format!(
                "replay of {} task(s): baseline v{} {b_solved} solved for ${b_cost:.4}, candidate {c_solved} solved for ${c_cost:.4}",
                ids.len(),
                baseline.version
            )
        };
        if adopted {
            self.store()
                .execute(&format!(
                    "UPDATE functions SET adopted = false WHERE name = {} AND adopted",
                    s(&cand.name)
                ))
                .await?;
            self.store()
                .execute(&format!(
                    "UPDATE functions SET adopted = true, eval_note = {} WHERE version = {candidate}",
                    s(&note)
                ))
                .await?;
        } else {
            self.store()
                .execute(&format!(
                    "UPDATE functions SET eval_note = {} WHERE version = {candidate}",
                    s(&format!("rejected: {note}"))
                ))
                .await?;
        }
        Ok((adopted, note))
    }

    /// Withdraw a function version (`kleene learn revert --function`); the
    /// newest earlier version of the same name is adopted again, so the
    /// before/after snapshot rolls back rather than leaving a gap.
    pub async fn revert_function(&self, version: i64) -> Result<Option<i64>, HarnessError> {
        let v = self.function_version(version).await?;
        self.store()
            .execute(&format!(
                "UPDATE functions SET adopted = false, eval_note = {} WHERE version = {version}",
                s("reverted by hand")
            ))
            .await?;
        if !v.adopted {
            return Ok(None);
        }
        let prev = self
            .store()
            .query(&format!(
                "SELECT version FROM functions WHERE name = {} AND version < {version} ORDER BY version DESC LIMIT 1",
                s(&v.name)
            ))
            .await?;
        if prev.rows.is_empty() {
            return Ok(None);
        }
        let p = int_at(&prev, 0, 0);
        self.store()
            .execute(&format!(
                "UPDATE functions SET adopted = true WHERE version = {p}"
            ))
            .await?;
        Ok(Some(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_extracted_from_replies_and_scripts() {
        let d = extract_definition(
            "Sure.\n```sql\nCREATE OR REPLACE FUNCTION v(x TEXT) RETURNS BOOLEAN AS PROMPT 'Is {x} ok?';\n```",
        )
        .unwrap();
        assert!(d.starts_with("CREATE OR REPLACE FUNCTION v"), "{d}");
        assert!(!d.ends_with(';'));
        assert!(extract_definition("no sql here").is_none());
        let defs = prompt_definitions(
            "CREATE TABLE t (x TEXT); CREATE FUNCTION a(x TEXT) RETURNS TEXT AS PROMPT 'A {x}'; CREATE FUNCTION b(x TEXT) RETURNS TEXT AS SHELL 'echo {x}'; SELECT 1",
        );
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].0, "a");
    }
}
