//! One module per tool. Each exposes a unit struct implementing [`Tool`].
//!
//! [`Tool`]: crate::Tool

pub mod append_file;
pub mod chunks;
pub mod convert;
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
pub mod read_csv;
pub mod read_json;
pub mod remove;
pub mod search;
pub mod shell;
pub mod skill;
pub mod web_fetch;
pub mod web_search;
pub mod write_file;

mod git;

use std::path::Path;
use std::sync::OnceLock;
use tokio::process::Command;

/// The command interpreter that `shell`, `--check`, pack setup commands and
/// shell oracles run a command line through: `("sh", "-c")` wherever `sh`
/// is on the `PATH`, which is every Unix and a Windows with Git Bash or
/// MSYS on the `PATH`, else `("cmd", "/C")` on Windows (issue #78). Decided
/// once per process.
pub fn shell_interpreter() -> (&'static str, &'static str) {
    static CHOSEN: OnceLock<(&'static str, &'static str)> = OnceLock::new();
    *CHOSEN.get_or_init(|| {
        if cfg!(windows) && !on_path("sh") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        }
    })
}

/// Whether `program` (with `.exe` on Windows) is in a `PATH` directory.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let name = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_string()
    };
    std::env::split_paths(&path).any(|dir| dir.join(&name).is_file())
}

/// A command line run through [`shell_interpreter`] in `cwd`, with the
/// daemon's environment inherited; the `shell` tool clears it instead
/// (see `clean_command`).
pub fn shell_command(command_line: &str, cwd: &Path) -> Command {
    let (program, flag) = shell_interpreter();
    let mut cmd = Command::new(program);
    cmd.arg(flag).arg(command_line).current_dir(cwd);
    cmd
}

/// Shell output as text: UTF-8, or UTF-16LE when it carries a byte-order
/// mark or every other byte of its first characters is zero (PowerShell's
/// default for a redirect), and NUL bytes dropped either way, because
/// DuckDB refuses a string that contains one (issue #78).
pub fn decode_output(bytes: &[u8]) -> String {
    let text = if bytes.starts_with(&[0xFF, 0xFE]) || looks_utf16le(bytes) {
        let start = usize::from(bytes.starts_with(&[0xFF, 0xFE])) * 2;
        let units: Vec<u16> = bytes[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    if text.contains('\0') {
        text.replace('\0', "")
    } else {
        text
    }
}

/// ASCII text in UTF-16LE: at least two characters, an even length, and a
/// zero high byte after each of the first eight (or all) low bytes, which
/// are printable or whitespace.
fn looks_utf16le(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return false;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .take(8)
        .all(|c| c[1] == 0 && (c[0].is_ascii_graphic() || c[0].is_ascii_whitespace()))
}

/// A subprocess with a minimal environment: everything cleared except
/// `PATH`, `HOME` and `LANG` (and, on Windows, what `cmd` and programs
/// cannot start without: `SYSTEMROOT`, `COMSPEC`, `PATHEXT`, `TEMP`, `TMP`,
/// `USERPROFILE`), so a session cannot read secrets out of the daemon's
/// environment through a child process.
pub(crate) fn clean_command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for key in ["PATH", "HOME", "LANG"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    if cfg!(windows) {
        for key in [
            "SYSTEMROOT",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "USERPROFILE",
        ] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
    }
    cmd.current_dir(cwd);
    cmd.kill_on_drop(true);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interpreter_is_sh_wherever_sh_exists() {
        let (program, flag) = shell_interpreter();
        if cfg!(windows) && !on_path("sh") {
            assert_eq!((program, flag), ("cmd", "/C"));
        } else {
            assert_eq!((program, flag), ("sh", "-c"));
        }
    }

    #[test]
    fn output_is_decoded_from_utf8_or_utf16le_without_nul_bytes() {
        assert_eq!(decode_output(b"hi\n"), "hi\n");
        assert_eq!(decode_output("ルート\n".as_bytes()), "ルート\n");
        // PowerShell's redirect: a BOM and UTF-16LE.
        let mut le = vec![0xFF, 0xFE];
        for u in "ルート\r\n".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_output(&le), "ルート\r\n");
        // The same without a BOM, recognised by the zero high bytes.
        let mut le = vec![];
        for u in "hello world\r\n".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_output(&le), "hello world\r\n");
        // Stray NULs in UTF-8 are dropped rather than refused by the store.
        assert_eq!(decode_output(b"a\0b\0"), "ab");
        // Binary that is not text stays lossy UTF-8, NUL-free.
        assert_eq!(decode_output(&[0xC3, 0x28, 0x00]), "\u{FFFD}(");
        assert!(!looks_utf16le(b"ab"), "too short");
        assert!(!looks_utf16le(b"abc"), "odd");
    }
}
