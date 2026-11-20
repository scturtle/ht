//! Host color queries and input handoff after report producers have stopped.

use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::raw_input::RawInputByteFramer;
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
    write_host_query(writer, query.as_bytes())
}

pub(super) fn write_host_query(writer: &mut impl io::Write, query: &[u8]) -> io::Result<()> {
    writer.write_all(query)?;
    writer.flush()
}

/// An unknown XTGETTCAP capability echoes its name in the negative response.
/// Unlike a plain DSR 0n, this acknowledgement identifies THIS shutdown, not a
/// stale/unrelated status response already in flight. Kitty/xterm implement it.
pub(super) struct ShutdownFence {
    pub(super) query: Vec<u8>,
    pub(super) reply: Vec<u8>,
}

impl ShutdownFence {
    pub(super) fn new(token: &str) -> Self {
        use std::fmt::Write as _;
        let mut name = String::new();
        for byte in token.bytes() {
            let _ = write!(name, "{byte:02x}");
        }
        Self {
            query: format!("\x1bP+q{name}\x1b\\").into_bytes(),
            reply: format!("\x1bP0+r{name}\x1b\\").into_bytes(),
        }
    }

    fn fresh() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self::new(&format!(
            "ht-{}-{nanos}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

#[derive(Debug)]
pub(super) struct DrainCompletion {
    pub(super) fence_received: bool,
    pub(super) pending_colors: u64,
}

impl DrainCompletion {
    pub(super) fn confirmed(&self) -> bool {
        self.fence_received && self.pending_colors == 0
    }
}

/// The UI and reader have ended and restoration disabled input producers, but
/// the tty is still raw. Require both a fresh, identifiable acknowledgement and
/// all registered color-query attempts. No input-parser timeout erases debt.
pub(in crate::client) fn drain_pending_host_replies(
    fd: RawFd,
    framer: &mut RawInputByteFramer,
    expected_colors: u64,
) {
    let started = Instant::now();
    let fence = ShutdownFence::fresh();
    let result = drain_host_replies_with(
        &mut io::stdout(),
        framer,
        expected_colors,
        &fence,
        Instant::now,
        |scratch, wait| {
            let timeout_ms = wait.as_millis().clamp(1, i32::MAX as u128) as i32;
            if !crate::platform::poll_fd_readable(fd, timeout_ms)? {
                return Ok(None);
            }
            crate::platform::read_fd(fd, scratch).map(Some)
        },
    );
    match result {
        Ok(completion) => tracing::info!(
            event = "terminal.shutdown_fence",
            confirmed = completion.confirmed(),
            fence_received = completion.fence_received,
            pending_colors = completion.pending_colors,
            expected_colors,
            received_colors = framer.host_color_replies_received(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            pid = std::process::id(),
            "host terminal input handoff"
        ),
        Err(err) => tracing::warn!(
            event = "terminal.shutdown_fence",
            confirmed = false,
            pending_colors = expected_colors.saturating_sub(framer.host_color_replies_received()),
            error = %err,
            "host reply drain unavailable during shutdown"
        ),
    }
}

pub(super) fn drain_host_replies_with(
    writer: &mut impl io::Write,
    framer: &mut RawInputByteFramer,
    expected_colors: u64,
    fence: &ShutdownFence,
    mut now: impl FnMut() -> Instant,
    mut read: impl FnMut(&mut [u8], Duration) -> io::Result<Option<usize>>,
) -> io::Result<DrainCompletion> {
    const MAX_SHUTDOWN_DRAIN: Duration = Duration::from_secs(2);
    write_host_query(writer, &fence.query)?;
    let deadline = now() + MAX_SHUTDOWN_DRAIN;
    let mut completion = DrainCompletion {
        fence_received: false,
        pending_colors: expected_colors.saturating_sub(framer.host_color_replies_received()),
    };
    let mut scratch = [0u8; 4096];
    while let Some(remaining) = deadline
        .checked_duration_since(now())
        .filter(|remaining| !remaining.is_zero())
    {
        match read(&mut scratch, remaining) {
            Ok(None | Some(0)) => break,
            Ok(Some(count)) => {
                // Compare whole framed sequences: paste/control-string lookalikes
                // cannot acknowledge the fence. Still consume colors AFTER ACK.
                for chunk in framer.push_without_idle_expiry(&scratch[..count]) {
                    completion.fence_received |= chunk == fence.reply;
                }
                completion.pending_colors =
                    expected_colors.saturating_sub(framer.host_color_replies_received());
                if completion.confirmed() {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(completion)
}
