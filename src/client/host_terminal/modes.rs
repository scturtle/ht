//! Host mouse and keyboard mode sequences; no pane terminal state lives here.

use std::io::{self, Write};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;

pub(super) fn ime_compatible_keyboard_enhancement_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModifyOtherKeysMode {
    Mode1,
    Mode2,
}

impl ModifyOtherKeysMode {
    pub(super) fn set_sequence(self) -> &'static [u8] {
        match self {
            Self::Mode1 => b"\x1b[>4;1m",
            Self::Mode2 => b"\x1b[>4;2m",
        }
    }
}

pub(super) fn host_modify_other_keys_mode() -> Option<ModifyOtherKeysMode> {
    let alacritty_window_id = false;

    host_modify_other_keys_mode_for_env(
        std::env::var("TMUX").is_ok(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var_os("WEZTERM_PANE").is_some(),
        alacritty_window_id,
    )
}

pub(super) fn host_modify_other_keys_mode_for_env(
    in_tmux: bool,
    term_program: Option<&str>,
    wezterm_pane: bool,
    alacritty_window_id: bool,
) -> Option<ModifyOtherKeysMode> {
    if in_tmux {
        return Some(ModifyOtherKeysMode::Mode2);
    }

    if wezterm_pane
        || alacritty_window_id
        || term_program.is_some_and(|program| program.eq_ignore_ascii_case("wezterm"))
    {
        return Some(ModifyOtherKeysMode::Mode1);
    }

    None
}

// Stop event generation before changing encodings. Resetting SGR first can
// turn an in-flight wheel/motion event into a legacy X10 report during cleanup.
const DISABLE_HOST_MOUSE_REPORTING_SEQUENCE: &[u8] =
    b"\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?9l\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l";

pub(in crate::client) fn clear_host_mouse_reporting<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE)?;
    writer.flush()
}

pub(in crate::client) fn set_host_kitty_keyboard_report_all<W: Write>(
    writer: &mut W,
    report_all_keys: bool,
) -> io::Result<()> {
    let mut flags = ime_compatible_keyboard_enhancement_flags();
    if report_all_keys {
        flags |= crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        flags = crossterm::event::KeyboardEnhancementFlags::from_bits_retain(
            flags.bits() | 0b0001_0000,
        );
    }
    crossterm::execute!(
        writer,
        crossterm::event::PopKeyboardEnhancementFlags,
        crossterm::event::PushKeyboardEnhancementFlags(flags)
    )
}

pub(in crate::client) fn effective_mouse_capture(
    server_enabled: bool,
    local_preference: bool,
) -> bool {
    server_enabled || local_preference
}

pub(in crate::client) fn effective_sgr_pixel_mouse(
    enabled: bool,
    requested: bool,
    pixel_mouse_available: bool,
) -> bool {
    enabled && requested && pixel_mouse_available
}

pub(in crate::client) fn set_mouse_capture(enabled: bool, sgr_pixels: bool) -> io::Result<()> {
    set_mouse_capture_with(&mut io::stdout(), enabled, sgr_pixels)
}

pub(super) fn set_mouse_capture_with(
    writer: &mut impl Write,
    enabled: bool,
    sgr_pixels: bool,
) -> io::Result<()> {
    clear_host_mouse_reporting(writer)?;
    if enabled {
        execute!(&mut *writer, EnableMouseCapture)?;
        if sgr_pixels {
            writer.write_all(b"\x1b[?1016h")?;
            writer.flush()?;
        }
        Ok(())
    } else {
        execute!(&mut *writer, DisableMouseCapture)
    }
}

pub(super) fn push_keyboard_enhancement_flags(writer: &mut impl Write) -> io::Result<()> {
    execute!(
        writer,
        PushKeyboardEnhancementFlags(ime_compatible_keyboard_enhancement_flags())
    )
}

pub(super) fn pop_keyboard_enhancement_flags(writer: &mut impl Write) -> io::Result<()> {
    execute!(writer, PopKeyboardEnhancementFlags)
}

/// Host input modes to re-assert after the host may have reset them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HostKeyboardProtocolRefresh {
    /// Some when the host confirmed the kitty keyboard protocol at startup;
    /// the value says whether all keys are currently reported as escapes.
    pub(super) kitty_report_all_keys: Option<bool>,
    pub(super) modify_other_keys_sequence: Option<&'static [u8]>,
    /// Re-enable focus reporting (1004). Callers reacting to a focus-in skip
    /// it: that host still reports focus, and re-enabling from a focus report
    /// could loop on a host that answers the enable with another report.
    pub(super) focus_reporting: bool,
}

pub(super) fn write_host_keyboard_protocol_refresh(
    writer: &mut impl io::Write,
    refresh: HostKeyboardProtocolRefresh,
) -> io::Result<()> {
    if let Some(report_all_keys) = refresh.kitty_report_all_keys {
        // Replace Herdr's stack entry rather than pushing, so repeated
        // refreshes never grow the host's keyboard stack. On a host that
        // reset its stack, the pop is a no-op and the push restores the flags.
        set_host_kitty_keyboard_report_all(writer, report_all_keys)?;
    }
    if let Some(sequence) = refresh.modify_other_keys_sequence {
        writer.write_all(sequence)?;
    }
    writer.write_all(b"\x1b[?2004h")?;
    if refresh.focus_reporting {
        writer.write_all(b"\x1b[?1004h")?;
    }
    writer.flush()
}
