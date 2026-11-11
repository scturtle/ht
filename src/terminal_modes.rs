use std::io::{self, Write};

const DISABLE_HOST_MOUSE_REPORTING_SEQUENCE: &[u8] =
    b"\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?9l";

#[cfg(test)]
const WINDOWS_MOUSE_REPORTING_ENABLE_SEQUENCE: &[u8] =
    b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";
#[cfg(test)]
const WINDOWS_MOUSE_REPORTING_DISABLE_SEQUENCE: &[u8] =
    b"\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

pub(crate) fn clear_host_mouse_reporting<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE)?;
    writer.flush()
}

#[cfg(test)]
pub(crate) fn set_windows_mouse_reporting<W: Write>(
    writer: &mut W,
    enabled: bool,
    sgr_pixels: bool,
) -> io::Result<()> {
    if enabled {
        // Keep a true-to-false pixel transition in the same buffered reassertion,
        // before the final SGR enable, so refresh never exposes a disabled gap.
        if !sgr_pixels {
            writer.write_all(b"\x1b[?1016l")?;
        }
        writer.write_all(WINDOWS_MOUSE_REPORTING_ENABLE_SEQUENCE)?;
        if sgr_pixels {
            writer.write_all(b"\x1b[?1016h")?;
        }
    } else {
        writer.write_all(WINDOWS_MOUSE_REPORTING_DISABLE_SEQUENCE)?;
    }
    writer.flush()
}

pub(crate) fn set_host_kitty_keyboard_report_all<W: Write>(
    writer: &mut W,
    report_all_keys: bool,
) -> io::Result<()> {
    let mut flags = crate::input::ime_compatible_keyboard_enhancement_flags();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keyboard_report_all_replaces_the_current_herdr_stack_entry() {
        let mut output = Vec::new();

        set_host_kitty_keyboard_report_all(&mut output, true).unwrap();
        set_host_kitty_keyboard_report_all(&mut output, false).unwrap();

        assert_eq!(output, b"\x1b[<1u\x1b[>31u\x1b[<1u\x1b[>7u");
    }

    #[test]
    fn clears_all_known_host_mouse_modes() {
        let mut output = Vec::new();
        clear_host_mouse_reporting(&mut output).unwrap();
        let sequence = std::str::from_utf8(&output).unwrap();

        for mode in ["9", "1000", "1002", "1003", "1005", "1006", "1015", "1016"] {
            assert!(
                sequence.contains(&format!("\x1b[?{mode}l")),
                "missing mouse mode {mode}"
            );
        }
    }

    #[test]
    fn windows_mouse_reporting_setup_and_teardown_request_required_modes() {
        let mut output = Vec::new();

        set_windows_mouse_reporting(&mut output, true, true).unwrap();
        set_windows_mouse_reporting(&mut output, true, false).unwrap();
        set_windows_mouse_reporting(&mut output, false, false).unwrap();

        assert_eq!(
            output,
            b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[?1016l\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
        );
    }
}
