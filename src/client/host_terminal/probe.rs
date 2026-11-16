//! Startup capability queries and response filtering; preserve unrelated host input.

use std::io::{self, Write as _};
use std::os::fd::AsRawFd as _;
use std::time::{Duration, Instant};

const HOST_CAPABILITY_QUERY_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_BUFFERED_HOST_INPUT: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct HostKeyboardProbeResponses {
    pub(super) flags: Option<u16>,
    pub(super) sgr_pixel_mouse_status: Option<u8>,
    pub(super) primary_device_attributes: bool,
}

/// What the startup probe learned about the host terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HostTerminalProbe {
    pub(super) escape_disambiguation: bool,
    /// None when the host did not answer; exact pixel input stays disabled.
    pub(super) sgr_pixel_mouse: Option<bool>,
}

pub(super) fn query_host_capabilities() -> (HostTerminalProbe, Vec<u8>) {
    const QUERY: &[u8] = b"\x1b[?1016$p\x1b[?u\x1b[c";

    let mut buffered_input = Vec::new();
    if let Err(err) = io::stdout()
        .write_all(QUERY)
        .and_then(|()| io::stdout().flush())
    {
        tracing::debug!(%err, "host capability query unavailable");
        return (HostTerminalProbe::default(), buffered_input);
    }

    // Bypass StdinLock's shared buffer so poll and read observe the same bytes.
    let stdin = io::stdin();
    let stdin_fd = stdin.as_raw_fd();
    let deadline = Instant::now() + HOST_CAPABILITY_QUERY_TIMEOUT;
    let mut responses = HostKeyboardProbeResponses::default();
    while !responses.primary_device_attributes && buffered_input.len() < MAX_BUFFERED_HOST_INPUT {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        match crate::platform::poll_fd_readable(stdin_fd, timeout_ms) {
            Ok(true) => {}
            Ok(false) => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                tracing::debug!(%err, "host keyboard enhancement query read unavailable");
                break;
            }
        }

        let mut scratch = [0u8; 4096];
        let capacity = MAX_BUFFERED_HOST_INPUT - buffered_input.len();
        let read_limit = capacity.min(scratch.len());
        match crate::platform::read_fd(stdin_fd, &mut scratch[..read_limit]) {
            Ok(0) => break,
            Ok(read) => {
                buffered_input.extend_from_slice(&scratch[..read]);
                consume_host_keyboard_probe_responses(&mut buffered_input, &mut responses);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                tracing::debug!(%err, "host keyboard enhancement query read failed");
                break;
            }
        }
    }

    (host_terminal_probe(&responses), buffered_input)
}

pub(super) fn host_terminal_probe(responses: &HostKeyboardProbeResponses) -> HostTerminalProbe {
    let sgr_pixel_mouse = match responses.sgr_pixel_mouse_status {
        // The query follows an explicit reset, so a host still reporting SET may
        // send pixels where Herdr expects cells. Wrong mouse units beat refusing
        // to start, so keep pixel mouse off and continue.
        Some(status @ (1 | 3)) => {
            tracing::warn!(status, "host SGR pixel mouse mode stays set after reset");
            Some(false)
        }
        Some(2) => Some(true),
        Some(0 | 4) => Some(false),
        _ => None,
    };
    HostTerminalProbe {
        escape_disambiguation: host_escape_disambiguation_confirmed(responses),
        sgr_pixel_mouse,
    }
}

pub(super) fn host_escape_disambiguation_confirmed(responses: &HostKeyboardProbeResponses) -> bool {
    responses.primary_device_attributes
        && responses
            .flags
            .is_some_and(|flags| flags & 0b0000_0001 != 0)
}

pub(super) fn consume_host_keyboard_probe_responses(
    buffered_input: &mut Vec<u8>,
    responses: &mut HostKeyboardProbeResponses,
) {
    const PASTE_START: &[u8] = b"\x1b[200~";
    const PASTE_END: &[u8] = b"\x1b[201~";

    let mut offset = 0;
    while offset < buffered_input.len() {
        if buffered_input[offset..].starts_with(PASTE_START) {
            let payload_start = offset + PASTE_START.len();
            let Some(relative_end) = buffered_input[payload_start..]
                .windows(PASTE_END.len())
                .position(|bytes| bytes == PASTE_END)
            else {
                break;
            };
            offset = payload_start + relative_end + PASTE_END.len();
            continue;
        }
        if let Some(control_string_end) = host_control_string_end(&buffered_input[offset..]) {
            let Some(control_string_end) = control_string_end else {
                break;
            };
            offset += control_string_end;
            continue;
        }
        if !buffered_input[offset..].starts_with(b"\x1b[?") {
            offset += 1;
            continue;
        }

        let start = offset;
        let mut end = start + 3;
        while end < buffered_input.len()
            && (buffered_input[end].is_ascii_digit() || buffered_input[end] == b';')
        {
            end += 1;
        }
        if end == buffered_input.len() {
            break;
        }
        let body = &buffered_input[start + 3..end];
        // DECRPM: CSI ? 1016 ; Ps $ y.
        if buffered_input[end] == b'$' {
            let Some(&final_byte) = buffered_input.get(end + 1) else {
                break;
            };
            if final_byte == b'y' {
                if let Some(status) = parse_sgr_pixel_mouse_status(body) {
                    responses.sgr_pixel_mouse_status = Some(status);
                    buffered_input.drain(start..=end + 1);
                    continue;
                }
            }
            offset += 1;
            continue;
        }
        let recognized = match buffered_input[end] {
            b'u' if !body.is_empty() && body.iter().all(u8::is_ascii_digit) => {
                std::str::from_utf8(body)
                    .ok()
                    .and_then(|flags| flags.parse().ok())
                    .map(|flags| {
                        if !responses.primary_device_attributes {
                            responses.flags = Some(flags);
                        }
                    })
                    .is_some()
            }
            b'c' if !body.is_empty()
                && body
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || *byte == b';') =>
            {
                responses.primary_device_attributes = true;
                true
            }
            _ => false,
        };
        if recognized {
            buffered_input.drain(start..=end);
        } else {
            offset += 1;
        }
    }
}

fn parse_sgr_pixel_mouse_status(body: &[u8]) -> Option<u8> {
    let mut fields = body.split(|byte| *byte == b';');
    (fields.next()? == b"1016").then_some(())?;
    let status = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
    (fields.next().is_none() && status <= 4).then_some(status)
}

fn host_control_string_end(bytes: &[u8]) -> Option<Option<usize>> {
    if bytes.first() != Some(&0x1b) {
        return None;
    }

    let allow_bel = if bytes.starts_with(b"\x1b]") {
        true
    } else if bytes
        .get(1)
        .is_some_and(|byte| matches!(*byte, b'P' | b'_' | b'^' | b'X'))
    {
        false
    } else {
        return None;
    };

    for offset in 2..bytes.len() {
        if allow_bel && bytes[offset] == 0x07 {
            return Some(Some(offset + 1));
        }
        if bytes[offset..].starts_with(b"\x1b\\") {
            return Some(Some(offset + 2));
        }
    }
    Some(None)
}
