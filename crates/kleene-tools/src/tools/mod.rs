//! One module per tool. Each exposes a unit struct implementing [`Tool`].
//!
//! [`Tool`]: crate::Tool

pub mod append_file;
pub mod chunks;
pub mod env;
pub mod files;
pub mod git_blame;
pub mod git_diff;
pub mod git_log;
pub mod grep;
pub mod lines;
pub mod mkdir;
pub mod patch;
pub mod read;
pub mod remove;
pub mod search;
pub mod shell;
pub mod skill;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;

mod git;

use std::path::Path;
use tokio::process::Command;

/// A subprocess with a minimal environment: everything cleared except
/// `PATH`, `HOME` and `LANG`, so a session cannot read secrets out of the
/// daemon's environment through a child process.
pub(crate) fn clean_command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for key in ["PATH", "HOME", "LANG"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.current_dir(cwd);
    cmd.kill_on_drop(true);
    cmd
}
