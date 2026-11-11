//! Presentation-only terminal state. Core is the sole PTY and process owner.
mod cursor;
mod input;
mod kitty_keyboard;
mod osc;
mod terminal;
#[cfg(test)]
pub(crate) mod testing;
mod xtgettcap;
#[cfg(test)]
pub(crate) use terminal::test_encode_key_for_app;
pub(crate) use terminal::{
    GhosttyPaneTerminal, PaneTerminal, ProcessBytesResult, ScrollMetrics, TerminalSearchDirection,
    TerminalTextPoint, TerminalWordMotion,
};
pub(crate) const PANE_TERM: &str = crate::ghostty::TERM;
pub(crate) const MIN_PANE_ROWS: u16 = 2;
pub(crate) const MIN_PANE_COLS: u16 = 4;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WheelRouting {
    MouseReport,
    AlternateScroll,
    HostScroll,
}
