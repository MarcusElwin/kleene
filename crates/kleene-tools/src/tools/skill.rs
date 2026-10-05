//! `skill(name TEXT)`: the body of a loaded skill.

use crate::args::{check_arity, text};
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `skill(name TEXT) -> (name TEXT, source TEXT, text TEXT)`, one row: the
/// Markdown body of the named skill from [`ToolContext::skills`]. An
/// unknown name is an argument error that lists the names there are.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillTool;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("name", DataType::Text),
                    Field::not_null("source", DataType::Text),
                    Field::not_null("text", DataType::Text),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "the full text of a skill listed under Skills, by name"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let name = text(args, 0, "name")?.trim().to_ascii_lowercase();
        match ctx
            .skills
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&name))
        {
            Some(s) => Ok(Batch {
                schema: self.schema(),
                rows: vec![vec![
                    Value::Text(s.name.clone()),
                    Value::Text(s.source.clone()),
                    Value::Text(s.body.clone()),
                ]],
            }),
            None => {
                let names: Vec<&str> = ctx.skills.iter().map(|s| s.name.as_str()).collect();
                Err(ToolError::Args(format!(
                    "no skill named {name:?}; skills: {}",
                    if names.is_empty() {
                        "(none)".to_string()
                    } else {
                        names.join(", ")
                    }
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::builtin;

    #[tokio::test]
    async fn returns_the_body_or_lists_the_names() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf()).with_skills(builtin());
        let b = SkillTool
            .call(&[Value::from("Code-Task")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows[0][0].render(), "code-task");
        assert!(b.rows[0][2].render().contains("CALL shell"));
        let err = SkillTool
            .call(&[Value::from("nope")], &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("create-skill"), "{err}");
    }
}
