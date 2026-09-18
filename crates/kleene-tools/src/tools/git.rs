//! Running `git` in the workspace, shared by the `git_*` tools.

use crate::paths::workspace_root;
use crate::tools::clean_command;
use crate::{ToolContext, ToolError};
use std::process::Stdio;

/// Run `git <args>` in the workspace root and return its stdout. A non-zero
/// exit becomes [`ToolError::Other`] carrying git's stderr; a missing `git`
/// binary surfaces as [`ToolError::Io`].
pub(crate) async fn git(ctx: &ToolContext, args: &[&str]) -> Result<String, ToolError> {
    let root = workspace_root(ctx)?;
    let out = clean_command("git", &root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(ToolError::Other(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Test support: a workspace with a git repository holding one commit of
/// `a.txt`, or `None` when `git` is not installed (the caller skips).
#[cfg(test)]
pub(crate) fn test_repo() -> Option<(tempfile::TempDir, ToolContext)> {
    use std::process::Command;
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Test Author",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), "first line\nsecond line\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "initial commit\n\nwith a body"]);
    let ctx = ToolContext::new(dir.path().to_path_buf());
    Some((dir, ctx))
}

#[cfg(test)]
pub(crate) fn commit_all(dir: &std::path::Path, message: &str) {
    use std::process::Command;
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", message]] {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Second Author",
                "-c",
                "user.email=second@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(&args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
