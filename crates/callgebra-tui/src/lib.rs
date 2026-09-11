//! Terminal UI.
//!
//! A thin ratatui client over the daemon protocol with five views: session
//! (call tree, transcript, streaming pane, plan sidebar), plan, trace
//! explorer, tasks board and help. Implemented in M4; ratatui is added then so
//! M0–M3 builds stay fast.

#![forbid(unsafe_code)]

/// Views the TUI can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Call tree, transcript, plan sidebar.
    Session,
    /// Full-width plan with alternatives.
    Plan,
    /// SQL over the trace database.
    Trace,
    /// Continual harness task board.
    Tasks,
    /// Keymap.
    Help,
}
