//! Host theme/appearance queries and platform-specific palette-query policy.

use std::io;

use crate::terminal::{HOST_COLOR_QUERY_SEQUENCE, HOST_COLOR_SCHEME_QUERY_SEQUENCE};

pub(in crate::client) fn query_host_terminal_appearance() {
    let _ = write_host_query(
        &mut io::stdout(),
        HOST_COLOR_SCHEME_QUERY_SEQUENCE.as_bytes(),
    );
}

pub(in crate::client) fn query_host_terminal_theme() {
    let _ = write_host_theme_query(
        &mut io::stdout(),
        crate::platform::should_query_host_terminal_palette(),
    );
}

pub(super) fn write_host_theme_query(
    writer: &mut impl io::Write,
    query_palette: bool,
) -> io::Result<()> {
    use std::fmt::Write as _;

    let mut query = String::from(HOST_COLOR_QUERY_SEQUENCE);
    if query_palette {
        for index in 0..=u8::MAX {
            let _ = write!(query, "\x1b]4;{index};?\x1b\\");
        }
    }
    write_host_query(writer, query.as_bytes())
}

pub(super) fn write_host_query(writer: &mut impl io::Write, query: &[u8]) -> io::Result<()> {
    writer.write_all(query)?;
    writer.flush()
}
