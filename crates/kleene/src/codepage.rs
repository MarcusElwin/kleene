//! The Windows console's code page (issue #72).
//!
//! Rust writes UTF-16 to a console, so text on the screen is right whatever
//! the code page; it is output that PowerShell captures or pipes that comes
//! out as mojibake on a cp932 or cp1252 console, because PowerShell decodes
//! the UTF-8 bytes with the console's code page. [`utf8`] switches the
//! console to UTF-8 (code page 65001) for as long as `kleene` runs and
//! restores the previous code page on the way out, so `kleene run ... |
//! Tee-Object` and `> log.txt` carry what the model wrote. When stdout is
//! a pipe and the console is still not UTF-8 (a detached process, or the
//! switch refused), [`note_output`] prints one hint on stderr the first
//! time non-ASCII text goes out.
//!
//! This module is the one place in the workspace that calls a foreign
//! function: the four `kernel32` code-page calls take and return plain
//! integers and touch no memory, which is why the crate allows `unsafe`
//! here and nowhere else. Everything is a no-op off Windows.

use std::sync::atomic::{AtomicBool, Ordering};

/// UTF-8, the code page `kleene` wants.
pub const UTF8: u32 = 65001;

/// Holds the console on UTF-8; dropping it restores the code pages that
/// were set before. [`restore`] does the same for the paths that end the
/// process without unwinding.
pub struct Utf8Console {
    _private: (),
}

impl Drop for Utf8Console {
    fn drop(&mut self) {
        restore();
    }
}

/// Printed once by [`note_output`].
static HINTED: AtomicBool = AtomicBool::new(false);

/// Switch the console to UTF-8 for the life of the guard when stdout is a
/// console whose code page is something else. `None` when there is nothing
/// to do: not Windows, no console, already UTF-8, or the switch refused.
pub fn utf8() -> Option<Utf8Console> {
    imp::switch().then_some(Utf8Console { _private: () })
}

/// Put back the code pages [`utf8`] replaced, if it did. Safe to call more
/// than once; a no-op when nothing was switched.
pub fn restore() {
    imp::restore();
}

/// The console's output code page (`chcp`'s number), or `None` without a
/// console or off Windows.
pub fn code_page() -> Option<u32> {
    imp::code_page()
}

/// Text is about to go to stdout. When stdout is a pipe, the console is
/// not UTF-8 and the text has a character outside ASCII, print one hint
/// on stderr saying how PowerShell will decode it and what to do.
pub fn note_output(text: &str) {
    if HINTED.load(Ordering::Relaxed) || text.is_ascii() {
        return;
    }
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        return;
    }
    let Some(cp) = code_page() else {
        return;
    };
    if cp == UTF8 || HINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "kleene: stdout is a pipe and the console code page is {cp}, so PowerShell will decode this UTF-8 output as cp{cp} and non-ASCII text will look garbled. Run [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 first, or write the answer with --json or --out."
    );
}

/// One line for `kleene setup --show`: the console code page and, when it
/// is not UTF-8, the PowerShell one-liner. Empty off Windows.
pub fn describe() -> String {
    match code_page() {
        None => String::new(),
        Some(UTF8) => format!("console      code page {UTF8} (UTF-8)\n"),
        Some(cp) => format!(
            "console      code page {cp}; kleene switches it to UTF-8 while it runs. For output piped from other programs: [Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n"
        ),
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::sync::Mutex;
    use windows_sys::Win32::System::Console::{
        GetConsoleCP, GetConsoleOutputCP, SetConsoleCP, SetConsoleOutputCP,
    };

    /// The (input, output) code pages to put back.
    static PREVIOUS: Mutex<Option<(u32, u32)>> = Mutex::new(None);

    pub fn code_page() -> Option<u32> {
        // SAFETY: GetConsoleOutputCP takes nothing and returns an integer;
        // 0 means the process has no console.
        let cp = unsafe { GetConsoleOutputCP() };
        (cp != 0).then_some(cp)
    }

    pub fn switch() -> bool {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() {
            return false;
        }
        let Some(out) = code_page() else {
            return false;
        };
        if out == super::UTF8 {
            return false;
        }
        // SAFETY: integer-only kernel32 calls on the attached console; a
        // zero return means the call was refused and nothing changed.
        let input = unsafe { GetConsoleCP() };
        let ok = unsafe { SetConsoleOutputCP(super::UTF8) } != 0;
        if !ok {
            return false;
        }
        let input_ok = unsafe { SetConsoleCP(super::UTF8) } != 0;
        if let Ok(mut p) = PREVIOUS.lock() {
            *p = Some((if input_ok { input } else { 0 }, out));
        }
        true
    }

    pub fn restore() {
        let previous = PREVIOUS.lock().ok().and_then(|mut p| p.take());
        if let Some((input, out)) = previous {
            // SAFETY: as in `switch`; failures leave the console as it is.
            unsafe {
                SetConsoleOutputCP(out);
                if input != 0 {
                    SetConsoleCP(input);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn code_page() -> Option<u32> {
        None
    }

    pub fn switch() -> bool {
        false
    }

    pub fn restore() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_windows_everything_is_a_no_op() {
        if cfg!(windows) {
            return;
        }
        assert!(utf8().is_none());
        assert_eq!(code_page(), None);
        assert_eq!(describe(), "");
        restore();
        note_output("ルート");
        assert!(!HINTED.load(Ordering::Relaxed));
    }

    #[test]
    fn the_hint_is_for_non_ascii_only() {
        note_output("plain ascii");
        assert!(!HINTED.load(Ordering::Relaxed));
    }
}
