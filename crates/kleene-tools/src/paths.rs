//! Path safety: every path a tool receives is confined to the workspace.
//!
//! [`resolve`] turns a model-supplied path into an absolute one under
//! [`ToolContext::workspace`] and refuses everything else with
//! [`ToolError::Denied`] naming the offending path:
//!
//! - relative paths are joined to the workspace root;
//! - absolute paths are accepted only if they lie under the workspace;
//! - any `..` component is rejected outright, even one that would stay
//!   inside;
//! - symlinks are followed: the longest existing prefix of the path is
//!   canonicalised, so a link that points outside the workspace is denied
//!   whether the link itself or something below it is named;
//! - for [`Access::Write`] the resolved path must additionally lie under one
//!   of the [`ToolContext::writable`] roots.
//!
//! The workspace root itself is canonicalised too, so a workspace reached
//! through a symlink (`/tmp` on macOS, say) still resolves correctly.

use crate::{ToolContext, ToolError};
use std::path::{Component, Path, PathBuf};

/// Whether the caller intends to read or to write the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read-only use: any path under the workspace.
    Read,
    /// Mutation: the path must also be under a writable root.
    Write,
}

/// A path that passed the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Absolute path with the existing prefix canonicalised.
    pub abs: PathBuf,
    /// The same path relative to the canonical workspace root, with `/`
    /// separators; what tools report back in `path` columns.
    pub rel: String,
}

/// Resolve `input` under the workspace, checking `access`.
pub fn resolve(ctx: &ToolContext, input: &str, access: Access) -> Result<Resolved, ToolError> {
    let root = workspace_root(ctx)?;
    let abs = confine(&root, input)?;
    if access == Access::Write && !is_writable(ctx, &root, &abs)? {
        return Err(ToolError::Denied(format!(
            "{input}: not in a writable path"
        )));
    }
    let rel = relative(&root, &abs);
    Ok(Resolved { abs, rel })
}

/// The canonical workspace root.
pub fn workspace_root(ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    ctx.workspace.canonicalize().map_err(|e| {
        ToolError::Denied(format!(
            "workspace {} is not accessible: {e}",
            ctx.workspace.display()
        ))
    })
}

/// A path relative to `root`, with forward slashes; empty for `root` itself.
pub fn relative(root: &Path, abs: &Path) -> String {
    let rel = abs.strip_prefix(root).unwrap_or(abs);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Join `input` to `root`, reject escapes, and canonicalise the existing
/// prefix so symlinks cannot lead outside.
fn confine(root: &Path, input: &str) -> Result<PathBuf, ToolError> {
    if input.is_empty() {
        return Err(ToolError::Args("empty path".into()));
    }
    let given = Path::new(input);
    let mut joined = root.to_path_buf();
    for c in given.components() {
        match c {
            Component::ParentDir => {
                return Err(ToolError::Denied(format!(
                    "{input}: `..` is not allowed in paths"
                )))
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                if !given.is_absolute() {
                    return Err(ToolError::Denied(format!("{input}: malformed path")));
                }
                joined = PathBuf::new();
                joined.push(c.as_os_str());
            }
            Component::Normal(part) => joined.push(part),
        }
    }
    if given.is_absolute() && !joined.starts_with(root) {
        return Err(ToolError::Denied(format!(
            "{input}: outside the workspace {}",
            root.display()
        )));
    }
    let canon = canonicalize_prefix(&joined)?;
    if !canon.starts_with(root) {
        return Err(ToolError::Denied(format!(
            "{input}: resolves outside the workspace {}",
            root.display()
        )));
    }
    Ok(canon)
}

/// Canonicalise the longest existing prefix of `path` and re-append the rest.
fn canonicalize_prefix(path: &Path) -> Result<PathBuf, ToolError> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(canon) => {
                let mut out = canon;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return Ok(out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    return Err(ToolError::Io(e));
                };
                tail.push(name);
                if !existing.pop() {
                    return Err(ToolError::Io(e));
                }
            }
            Err(e) => return Err(ToolError::Io(e)),
        }
    }
}

/// `true` if `abs` lies under one of the writable roots.
fn is_writable(ctx: &ToolContext, root: &Path, abs: &Path) -> Result<bool, ToolError> {
    for w in &ctx.writable {
        let base = if w.is_absolute() {
            w.clone()
        } else {
            root.join(w)
        };
        let base = match canonicalize_prefix(&base) {
            Ok(b) => b,
            Err(ToolError::Io(_)) => continue,
            Err(e) => return Err(e),
        };
        if abs.starts_with(&base) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ws() -> (TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        (dir, ctx)
    }

    fn denied(r: Result<Resolved, ToolError>) -> String {
        match r {
            Err(ToolError::Denied(msg)) => msg,
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn relative_paths_resolve_under_the_workspace() {
        let (dir, ctx) = ws();
        let r = resolve(&ctx, "src/a.rs", Access::Read).unwrap();
        assert_eq!(r.abs, dir.path().canonicalize().unwrap().join("src/a.rs"));
        assert_eq!(r.rel, "src/a.rs");
        let r = resolve(&ctx, "./src/new.txt", Access::Write).unwrap();
        assert_eq!(r.rel, "src/new.txt");
    }

    #[test]
    fn dotdot_is_denied_even_when_it_stays_inside() {
        let (_dir, ctx) = ws();
        assert!(denied(resolve(&ctx, "../etc/passwd", Access::Read)).contains("`..`"));
        assert!(denied(resolve(&ctx, "src/../src/a.rs", Access::Read)).contains("`..`"));
    }

    #[test]
    fn absolute_paths_outside_are_denied_and_inside_accepted() {
        let (dir, ctx) = ws();
        let msg = denied(resolve(&ctx, "/etc/passwd", Access::Read));
        assert!(msg.starts_with("/etc/passwd"), "{msg}");
        let inside = dir.path().join("src/a.rs");
        let r = resolve(&ctx, inside.to_str().unwrap(), Access::Read).unwrap();
        assert_eq!(r.rel, "src/a.rs");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_are_denied() {
        let (dir, ctx) = ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            dir.path().join("src/nested/leak"),
        )
        .unwrap();
        assert!(denied(resolve(&ctx, "link", Access::Read)).contains("outside"));
        assert!(denied(resolve(&ctx, "link/secret", Access::Read)).contains("outside"));
        assert!(denied(resolve(&ctx, "link/new-file", Access::Write)).contains("outside"));
        assert!(denied(resolve(&ctx, "src/nested/leak", Access::Read)).contains("outside"));
        // A link that stays inside is fine.
        std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("srclink")).unwrap();
        let r = resolve(&ctx, "srclink/a.rs", Access::Read).unwrap();
        assert_eq!(r.rel, "src/a.rs");
    }

    #[test]
    fn writes_need_a_writable_root() {
        let (_dir, mut ctx) = ws();
        ctx.writable = vec![PathBuf::from("src/nested")];
        assert!(resolve(&ctx, "src/nested/x.txt", Access::Write).is_ok());
        assert!(resolve(&ctx, "src/a.rs", Access::Read).is_ok());
        let msg = denied(resolve(&ctx, "src/a.rs", Access::Write));
        assert!(msg.contains("not in a writable path"), "{msg}");
        ctx.writable = vec![];
        assert!(resolve(&ctx, "src/nested/x.txt", Access::Write).is_err());
    }

    #[test]
    fn missing_workspace_is_denied() {
        let ctx = ToolContext::new(PathBuf::from("/definitely/not/here"));
        assert!(matches!(
            resolve(&ctx, "a", Access::Read),
            Err(ToolError::Denied(_))
        ));
    }
}
