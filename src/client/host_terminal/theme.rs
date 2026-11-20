//! Host terminal color queries.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::terminal::HOST_COLOR_QUERY_SEQUENCE;

pub(in crate::client) fn query_host_terminal_theme(expected: &AtomicU64) {
    let _ = write_host_theme_query(&mut io::stdout(), expected);
}

pub(super) fn write_host_theme_query(
    writer: &mut impl io::Write,
    expected: &AtomicU64,
) -> io::Result<()> {
    use std::fmt::Write as _;
    let mut query = String::from(HOST_COLOR_QUERY_SEQUENCE);
    for index in 0..=u8::MAX {
        let _ = write!(query, "\x1b]4;{index};?\x1b\\");
    }
    // Arm the reader before writing: two default colors and 256 palette entries.
    expected.fetch_add(2 + 256, Ordering::AcqRel);
    writer.write_all(query.as_bytes())?;
    writer.flush()
}
