use std::io::{self, Write};

const DISABLE_HOST_MOUSE_REPORTING_SEQUENCE: &[u8] =
    b"\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?9l";

pub(crate) fn clear_host_mouse_reporting<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE)?;
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
}
