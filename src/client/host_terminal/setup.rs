//! Terminal setup and restoration for the rendered client.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
};
use crossterm::execute;
use crossterm::terminal::{DisableLineWrap, EnableLineWrap};

use super::modes::{
    clear_host_mouse_reporting, pop_keyboard_enhancement_flags, push_keyboard_enhancement_flags,
    set_mouse_capture_with, write_host_keyboard_protocol_refresh, HostKeyboardProtocolRefresh,
};
use super::probe::{query_host_capabilities, HostTerminalProbe};
use crate::client::frame_output::clear_received_kitty_graphics;

// ---------------------------------------------------------------------------
// Terminal setup / restore
// ---------------------------------------------------------------------------

/// Sets up the terminal for client mode (raw mode, mouse capture, keyboard enhancements).
///
/// Returns a guard that restores the terminal when dropped.
pub(in crate::client) fn setup_terminal() -> io::Result<TerminalGuard> {
    ratatui::init();
    setup_terminal_with(
        TerminalGuard::new(),
        &mut io::stdout(),
        query_host_capabilities,
        || crate::input::host_modify_other_keys_mode().map(|mode| mode.set_sequence()),
    )
}

/// Adapter setup after Ratatui initialization. Inject I/O and queries so failures
/// can exercise the real progress flags and Drop without touching a live terminal.
pub(super) fn setup_terminal_with(
    mut terminal_guard: TerminalGuard,
    writer: &mut impl io::Write,
    probe: impl FnOnce() -> (HostTerminalProbe, Vec<u8>),
    modify_other_keys_sequence: impl FnOnce() -> Option<&'static [u8]>,
) -> io::Result<TerminalGuard> {
    clear_host_mouse_reporting(writer)?;
    terminal_guard.reset_keyboard_enhancements = true;
    push_keyboard_enhancement_flags(writer)?;
    let (host_probe, buffered_host_input) = probe();
    set_mouse_capture_with(writer, true, false)?;
    execute!(&mut *writer, EnableBracketedPaste, EnableFocusChange)?;
    terminal_guard.reset_host_color_scheme_reports = true;
    enable_host_color_scheme_reports(writer)?;
    if let Some(sequence) = modify_other_keys_sequence() {
        terminal_guard.modify_other_keys_sequence = Some(sequence);
        writer.write_all(sequence)?;
        writer.flush()?;
    }

    execute!(writer, DisableLineWrap)?;

    terminal_guard.host_escape_disambiguation_active = host_probe.escape_disambiguation;
    terminal_guard.host_sgr_pixel_mouse = host_probe.sgr_pixel_mouse;
    terminal_guard.buffered_host_input = buffered_host_input;
    Ok(terminal_guard)
}

/// Guard that restores the terminal when dropped.
pub(in crate::client) struct TerminalGuard {
    host_escape_disambiguation_active: bool,
    host_sgr_pixel_mouse: Option<bool>,
    buffered_host_input: Vec<u8>,
    reset_keyboard_enhancements: bool,
    /// The modifyOtherKeys sequence written at setup, re-sent on refresh.
    modify_other_keys_sequence: Option<&'static [u8]>,
    reset_host_color_scheme_reports: bool,
    // Shared attempt claim, not a success flag: failed I/O must not trigger another pop.
    restore_claimed: Arc<AtomicBool>,
    #[cfg(test)]
    test_restore: Option<Arc<TestRestore>>,
}

pub(super) fn enable_host_color_scheme_reports(writer: &mut impl io::Write) -> io::Result<()> {
    writer.write_all(crate::terminal::HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE.as_bytes())?;
    writer.flush()
}

pub(super) fn write_terminal_restore_postlude(
    writer: &mut impl io::Write,
    reset_host_color_scheme_reports: bool,
) -> io::Result<()> {
    if reset_host_color_scheme_reports {
        writer.write_all(crate::terminal::HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE.as_bytes())?;
    }
    // Restore a visible cursor and reset DECSCUSR back to the terminal default.
    writer.write_all(b"\x1b[?25h\x1b[0 q")?;
    writer.flush()
}

fn restore_terminal_state_once(
    restore_claimed: &AtomicBool,
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
    #[cfg(test)] test_restore: Option<&TestRestore>,
) -> io::Result<()> {
    // Claim before any I/O. An error (or panic) must never cause a second stack pop.
    if restore_claimed.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    #[cfg(test)]
    if let Some(restore) = test_restore {
        return restore(
            reset_keyboard_enhancements,
            reset_modify_other_keys,
            reset_host_color_scheme_reports,
        );
    }
    restore_terminal_state(
        reset_keyboard_enhancements,
        reset_modify_other_keys,
        reset_host_color_scheme_reports,
    )
}

fn restore_terminal_state(
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
) -> io::Result<()> {
    restore_terminal_state_with(
        &mut io::stdout(),
        reset_keyboard_enhancements,
        reset_modify_other_keys,
        reset_host_color_scheme_reports,
        |writer| clear_received_kitty_graphics(writer),
        ratatui::try_restore,
    )
}

/// Preserve best-effort mode cleanup, then always attempt Ratatui restore and
/// the color/cursor postlude. Ratatui's error takes precedence over the postlude.
pub(super) fn restore_terminal_state_with<W: io::Write>(
    writer: &mut W,
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
    clear_graphics: impl FnOnce(&mut W) -> io::Result<()>,
    restore_terminal: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let _ = clear_graphics(writer);

    if reset_modify_other_keys {
        let _ = writer.write_all(b"\x1b[>4;0m");
        let _ = writer.flush();
    }
    if reset_keyboard_enhancements {
        let _ = pop_keyboard_enhancement_flags(writer);
    }

    let _ = execute!(
        &mut *writer,
        EnableLineWrap,
        DisableFocusChange,
        DisableBracketedPaste
    );
    let _ = set_mouse_capture_with(writer, false, false);

    let restore_result = restore_terminal();
    let postlude_result = write_terminal_restore_postlude(writer, reset_host_color_scheme_reports);
    restore_result.and(postlude_result)
}

impl TerminalGuard {
    fn new() -> Self {
        Self {
            host_escape_disambiguation_active: false,
            host_sgr_pixel_mouse: None,
            buffered_host_input: Vec::new(),
            reset_keyboard_enhancements: false,
            modify_other_keys_sequence: None,
            reset_host_color_scheme_reports: false,
            restore_claimed: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            test_restore: None,
        }
    }

    fn restore_state(&self) -> io::Result<()> {
        restore_terminal_state_once(
            &self.restore_claimed,
            self.reset_keyboard_enhancements,
            self.modify_other_keys_sequence.is_some(),
            self.reset_host_color_scheme_reports,
            #[cfg(test)]
            self.test_restore.as_deref(),
        )
    }

    pub(in crate::client) fn host_escape_disambiguation_active(&self) -> bool {
        self.host_escape_disambiguation_active
    }

    /// Whether the host answered that it supports SGR pixel mouse reports.
    pub(in crate::client) fn host_sgr_pixel_mouse(&self) -> Option<bool> {
        self.host_sgr_pixel_mouse
    }

    pub(in crate::client) fn take_buffered_host_input(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffered_host_input)
    }

    /// Re-sends the keyboard and input modes enabled at setup. A host that
    /// reset its terminal state (a reconnect or window reload) forgets them,
    /// which turns Shift+Enter and Ctrl+Enter into plain Enter.
    pub(in crate::client) fn refresh_host_keyboard_protocol(
        &self,
        writer: &mut impl io::Write,
        report_all_keys: bool,
        focus_reporting: bool,
    ) -> io::Result<()> {
        // Direct attach leaves these modes to the attached pane.
        if !self.reset_keyboard_enhancements || self.restore_claimed.load(Ordering::Acquire) {
            return Ok(());
        }
        write_host_keyboard_protocol_refresh(
            writer,
            HostKeyboardProtocolRefresh {
                kitty_report_all_keys: self
                    .host_escape_disambiguation_active
                    .then_some(report_all_keys),
                modify_other_keys_sequence: self.modify_other_keys_sequence,
                focus_reporting,
            },
        )
    }

    /// Captures the restoration state for use by the process panic hook.
    pub(in crate::client) fn panic_restore(&self) -> impl Fn() + Send + Sync + 'static {
        let restore_claimed = self.restore_claimed.clone();
        let reset_keyboard_enhancements = self.reset_keyboard_enhancements;
        let reset_modify_other_keys = self.modify_other_keys_sequence.is_some();
        let reset_host_color_scheme_reports = self.reset_host_color_scheme_reports;
        #[cfg(test)]
        let test_restore = self.test_restore.clone();
        move || {
            let _ = restore_terminal_state_once(
                &restore_claimed,
                reset_keyboard_enhancements,
                reset_modify_other_keys,
                reset_host_color_scheme_reports,
                #[cfg(test)]
                test_restore.as_deref(),
            );
        }
    }

    pub(in crate::client) fn restore(self) -> io::Result<()> {
        self.restore_state()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore_state();
    }
}

// Per-guard test I/O only: no process-wide panic hook, stdout or raw-mode mutation.
#[cfg(test)]
type TestRestore = dyn Fn(bool, bool, bool) -> io::Result<()> + Send + Sync;

#[cfg(test)]
pub(super) fn terminal_guard_for_test(
    restore: impl Fn(bool, bool, bool) -> io::Result<()> + Send + Sync + 'static,
) -> TerminalGuard {
    let mut guard = TerminalGuard::new();
    guard.test_restore = Some(Arc::new(restore));
    guard
}

#[cfg(test)]
pub(super) fn refresh_test_guard(
    kitty_confirmed: bool,
    client_protocols: bool,
    modify_other_keys_sequence: Option<&'static [u8]>,
) -> TerminalGuard {
    let mut guard = terminal_guard_for_test(|_, _, _| Ok(()));
    guard.host_escape_disambiguation_active = kitty_confirmed;
    guard.reset_keyboard_enhancements = client_protocols;
    guard.modify_other_keys_sequence = modify_other_keys_sequence;
    guard
}

#[cfg(test)]
impl TerminalGuard {
    pub(super) fn mark_restore_claimed_for_test(&self) {
        self.restore_claimed.store(true, Ordering::Release);
    }
}
