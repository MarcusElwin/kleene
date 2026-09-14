//! `shell(cmd TEXT [, cwd TEXT, timeout_ms BIGINT])`: run a command.
//!
//! **No sandbox in M2.** The command runs as a plain `sh -c` subprocess of
//! the daemon with a cleared environment (only `PATH`, `HOME` and `LANG`
//! survive) and its working directory confined to the workspace. M3 wraps it
//! in `hakoniwa` (namespaces, tmpfs root, Landlock, seccomp, cgroups); until
//! then a role that may `CALL shell` can do anything the daemon's user can.

use crate::args::{check_arity, opt_int, opt_text, text};
use crate::paths::{resolve, workspace_root, Access};
use crate::tools::clean_command;
use crate::{Signature, Tool, ToolContext, ToolError};
use kleene_core::{Batch, DataType, Field, Schema, Value, Volatility};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Bytes kept of each of stdout and stderr; the rest is drained and counted.
pub const MAX_STREAM_BYTES: usize = 1024 * 1024;

/// Read a stream to its end, keeping the first [`MAX_STREAM_BYTES`] and a
/// note of how much was dropped.
async fn read_capped<R: AsyncRead + Unpin>(mut reader: R) -> String {
    let mut kept = Vec::new();
    let mut total = 0usize;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total += n;
                let room = MAX_STREAM_BYTES.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if total > MAX_STREAM_BYTES {
        text.push_str(&format!(
            "\n... truncated ({total} bytes total, first {MAX_STREAM_BYTES} shown)"
        ));
    }
    text
}

/// `shell(cmd TEXT [, cwd TEXT, timeout_ms BIGINT]) -> (stdout TEXT, stderr TEXT, exit_code BIGINT, duration_ms BIGINT)`.
///
/// `cwd` defaults to the workspace root and must be a directory inside it;
/// `timeout_ms` defaults to [`ToolContext::shell_timeout_ms`]. On timeout
/// the process is killed and the call fails with [`ToolError::Timeout`].
/// Each stream is capped at 1 MiB with a trailing `... truncated` marker.
/// `exit_code` is -1 when the process died from a signal.
#[derive(Debug, Default, Clone, Copy)]
pub struct Shell;

static SCHEMA: OnceLock<Arc<Schema>> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for Shell {
    fn name(&self) -> &str {
        "shell"
    }

    fn signature(&self) -> Signature {
        Signature::new(&[DataType::Text], &[DataType::Text, DataType::Int])
    }

    fn schema(&self) -> Arc<Schema> {
        SCHEMA
            .get_or_init(|| {
                Arc::new(Schema::new(vec![
                    Field::not_null("stdout", DataType::Text),
                    Field::not_null("stderr", DataType::Text),
                    Field::not_null("exit_code", DataType::Int),
                    Field::not_null("duration_ms", DataType::Int),
                ]))
            })
            .clone()
    }

    fn volatility(&self) -> Volatility {
        Volatility::Volatile
    }

    fn description(&self) -> &str {
        "run a command with sh -c in the workspace: stdout, stderr, exit_code, duration_ms"
    }

    async fn call(&self, args: &[Value], ctx: &ToolContext) -> Result<Batch, ToolError> {
        check_arity(self.name(), &self.signature(), args)?;
        let cmd = text(args, 0, "cmd")?;
        let cwd = match opt_text(args, 1, "cwd")? {
            None => workspace_root(ctx)?,
            Some(dir) => {
                let r = resolve(ctx, dir, Access::Read)?;
                if !r.abs.is_dir() {
                    return Err(ToolError::Args(format!("cwd {dir} is not a directory")));
                }
                r.abs
            }
        };
        let timeout_ms = match opt_int(args, 2, "timeout_ms")? {
            None => ctx.shell_timeout_ms,
            Some(t) if t > 0 => t as u64,
            Some(t) => {
                return Err(ToolError::Args(format!(
                    "timeout_ms must be positive, got {t}"
                )))
            }
        };

        let started = Instant::now();
        let mut child = clean_command("sh", &cwd)
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let out_task = tokio::spawn(read_capped(stdout));
        let err_task = tokio::spawn(read_capped(stderr));

        let status =
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(status) => status?,
                Err(_) => {
                    let _ = child.kill().await;
                    out_task.abort();
                    err_task.abort();
                    return Err(ToolError::Timeout(timeout_ms));
                }
            };
        let stdout = out_task.await.unwrap_or_default();
        let stderr = err_task.await.unwrap_or_default();
        let duration_ms = started.elapsed().as_millis() as i64;
        Ok(Batch {
            schema: self.schema(),
            rows: vec![vec![
                Value::Text(stdout),
                Value::Text(stderr),
                Value::Int(i64::from(status.code().unwrap_or(-1))),
                Value::Int(duration_ms),
            ]],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        (dir, ctx)
    }

    async fn run(ctx: &ToolContext, args: &[Value]) -> (String, String, i64) {
        let b = Shell.call(args, ctx).await.unwrap();
        let r = &b.rows[0];
        (r[0].render(), r[1].render(), r[2].as_int().unwrap())
    }

    #[tokio::test]
    async fn echo_and_exit_codes() {
        let (_d, ctx) = ws();
        let (out, err, code) = run(&ctx, &[Value::from("echo hi; echo oops >&2")]).await;
        assert_eq!(out, "hi\n");
        assert_eq!(err, "oops\n");
        assert_eq!(code, 0);
        let (_, _, code) = run(&ctx, &[Value::from("exit 3")]).await;
        assert_eq!(code, 3);
    }

    #[tokio::test]
    async fn runs_in_the_workspace_or_a_subdirectory() {
        let (dir, ctx) = ws();
        let root = dir.path().canonicalize().unwrap();
        let (out, _, _) = run(&ctx, &[Value::from("pwd")]).await;
        assert_eq!(out.trim(), root.to_str().unwrap());
        let (out, _, _) = run(&ctx, &[Value::from("pwd"), Value::from("sub")]).await;
        assert_eq!(out.trim(), root.join("sub").to_str().unwrap());
        let err = Shell
            .call(&[Value::from("pwd"), Value::from("../")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Denied(_)), "{err}");
        let err = Shell
            .call(&[Value::from("pwd"), Value::from("nope")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Args(_)), "{err}");
    }

    #[tokio::test]
    async fn times_out_and_kills() {
        let (_d, mut ctx) = ws();
        let started = Instant::now();
        let err = Shell
            .call(
                &[Value::from("sleep 5"), Value::Null, Value::Int(100)],
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Timeout(100)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
        ctx.shell_timeout_ms = 50;
        let err = Shell
            .call(&[Value::from("sleep 5")], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Timeout(50)), "{err}");
    }

    #[tokio::test]
    async fn caps_stdout_with_a_marker() {
        let (_d, ctx) = ws();
        let n = MAX_STREAM_BYTES + 100_000;
        let cmd = format!("head -c {n} /dev/zero | tr '\\0' a");
        let (out, _, code) = run(&ctx, &[Value::from(cmd)]).await;
        assert_eq!(code, 0);
        assert!(out.starts_with("aaaa"));
        assert!(
            out.contains(&format!("... truncated ({n} bytes total")),
            "{}",
            &out[out.len() - 80..]
        );
        assert!(out.len() < n);
    }

    #[tokio::test]
    async fn environment_is_cleared() {
        let (_d, ctx) = ws();
        // cargo sets CARGO_MANIFEST_DIR for tests; the child must not see it.
        let (out, _, _) = run(&ctx, &[Value::from("env")]).await;
        assert!(!out.contains("CARGO_MANIFEST_DIR"), "{out}");
        assert!(out.contains("PATH="));
    }
}
