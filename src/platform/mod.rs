// Platform support for the Core TUI: terminal I/O, input, files, and notifications.
pub(crate) use unix_common::{client_terminal_identity, poll_fd_readable, read_fd};

pub(crate) fn prepare_paste_text_for_pty(text: String) -> String {
    prepare_paste_text_for_pty_platform(text)
}

fn prepare_paste_text_for_pty_platform(text: String) -> String {
    text
}

#[cfg(test)]
pub(crate) fn configure_background_command(command: &mut std::process::Command) {
    configure_background_command_platform(command);
}

#[cfg(test)]
fn configure_background_command_platform(_command: &mut std::process::Command) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlatformCapabilities {
    pub(crate) live_handoff: bool,
    pub(crate) direct_terminal_attach: bool,
    pub(crate) preserve_legacy_doubled_escape_input: bool,
}

pub(crate) const fn capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        live_handoff: true,
        direct_terminal_attach: true,
        preserve_legacy_doubled_escape_input: false,
    }
}

/// The byte the host terminal sends for Backspace according to the tty's erase
/// setting (e.g. `^H` for MobaXterm and PuTTY-style terminals), when known.
pub(crate) fn terminal_erase_byte() -> Option<u8> {
    unix_common::terminal_erase_byte()
}

/// Error kind marker for a terminal that momentarily has no usable grid.
///
/// A window drag can sample a zero-sized grid before the terminal settles, so
/// this is distinct from a real terminal failure. Startup still rejects it; the
/// resize watcher treats it as a transient sample instead of a dead terminal.
#[derive(Debug)]
pub(crate) struct ZeroSizedGridError;

impl std::fmt::Display for ZeroSizedGridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("terminal reported a zero-sized grid")
    }
}

impl std::error::Error for ZeroSizedGridError {}

pub(crate) fn zero_sized_terminal_grid_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, ZeroSizedGridError)
}

/// Whether an error only means the terminal has no usable size yet, so a
/// resizing terminal should keep its last known geometry instead of detaching.
pub(crate) fn is_transient_terminal_grid_error(err: &std::io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<ZeroSizedGridError>())
}

pub(crate) fn terminal_grid_size() -> std::io::Result<(u16, u16)> {
    let (cols, rows) = unix_common::read_terminal_grid_size()?;

    if cols == 0 || rows == 0 {
        return Err(zero_sized_terminal_grid_error());
    }
    Ok((cols, rows))
}

/// Raised by the SIGWINCH handler, consumed by the host resize watcher.
static TERMINAL_RESIZE_SIGNALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

extern "C" fn record_terminal_resize_signal(_signal: libc::c_int) {
    TERMINAL_RESIZE_SIGNALLED.store(true, std::sync::atomic::Ordering::Release);
}

/// Records SIGWINCH events that size polling can miss.
pub(crate) fn watch_terminal_resize_signal() {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction =
        record_terminal_resize_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // Keep blocking stdin and socket reads from failing with EINTR.
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
    }
}

/// Returns whether a terminal size change was signalled since the last call.
pub(crate) fn take_terminal_resize_signal() -> bool {
    TERMINAL_RESIZE_SIGNALLED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardCommand {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

mod unix_common;

pub(crate) mod unix_image_files;

mod client_state;

pub(crate) use client_state::{create_private_state_file, replace_file};

mod linux;

pub use linux::*;

#[derive(Debug)]
pub(crate) struct InputSourceRestore;

pub(crate) fn switch_to_ascii_input_source() -> Option<InputSourceRestore> {
    None
}

pub(crate) fn pump_input_source_runloop() {}

/// Switches the host keyboard input source while prefix mode is active.
///
/// `App` drives this through a trait so the prefix-mode transitions can be
/// tested with a fake, without touching the real macOS APIs or leaking a
/// platform-specific restore type into `App`.
pub(crate) trait PrefixInputSource {
    /// Switch to an ASCII-capable input source for prefix commands. No-op if
    /// the current source is already ASCII-capable, the platform is
    /// unsupported, or the switch fails. Calling it again before `restore`
    /// keeps the source saved by the first call.
    fn switch_to_ascii(&mut self);

    /// Restore whatever `switch_to_ascii` saved. No-op if nothing was switched.
    fn restore(&mut self);
}

/// Production [`PrefixInputSource`] backed by the per-platform API.
#[derive(Default)]
pub(crate) struct RealPrefixInputSource {
    restore: Option<InputSourceRestore>,
}

impl PrefixInputSource for RealPrefixInputSource {
    fn switch_to_ascii(&mut self) {
        if self.restore.is_none() {
            // Drain pending input-source-change notifications so the read below is fresh (see
            // `pump_input_source_runloop`); a no-op on non-macOS.
            pump_input_source_runloop();
            self.restore = switch_to_ascii_input_source();
        }
    }

    fn restore(&mut self) {
        let _ = self.restore.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_resize_signal_is_recorded_once_per_delivery() {
        watch_terminal_resize_signal();
        assert!(!take_terminal_resize_signal());

        unsafe {
            libc::raise(libc::SIGWINCH);
        }

        assert!(take_terminal_resize_signal());
        assert!(!take_terminal_resize_signal());
    }
}
