//! `env(name TEXT)`: allow-listed environment variables.

use crate::args::{check_arity, text};
use crate::{Signature, Tool, ToolContext, ToolError};
use callgebra_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::sync::{Arc, OnceLock};

/// `env(name TEXT) -> (value TEXT)`, one row; `NULL` when the variable is
/// unset. Only names in [`ToolContext::env_allowlist`] may be read; anything
/// else is denied, so secrets in the daemon's environment stay out of reach.
#[derive(Debug, Default, Clone, Copy)]
pub struct Env;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Env {
    fn name(&self) -> &str {
        "env"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| Arc::new(Schema::new(vec![Field::new("value", DataType::Text)])))
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Stable
    }

    fn description(&self) -> &str {
        "the value of an allow-listed environment variable, NULL if unset"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let name = text(args, 0, "name")?;
        if !ctx.env_allowlist.iter().any(|n| n == name) {
            return Err(ToolError::Denied(format!(
                "environment variable {name} is not on the allow-list"
            )));
        }
        let value = std::env::var_os(name)
            .map(|v| Value::Text(v.to_string_lossy().into_owned()))
            .unwrap_or(Value::Null);
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![value]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn only_allow_listed_names_are_readable() {
        let mut ctx = ToolContext::new(PathBuf::from("."));
        let err = Env.call(&[Value::from("PATH")], &ctx).await.unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        ctx.env_allowlist = vec!["PATH".into(), "CALLGEBRA_SURELY_UNSET_VAR".into()];
        let b = Env.call(&[Value::from("PATH")], &ctx).await.unwrap();
        assert_eq!(b.len(), 1);
        assert!(!b.rows[0][0].is_null());
        let b = Env
            .call(&[Value::from("CALLGEBRA_SURELY_UNSET_VAR")], &ctx)
            .await
            .unwrap();
        assert_eq!(b.rows[0][0], Value::Null);
    }
}
