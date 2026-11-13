use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

/// Decode bytes the byte framer already split into complete input. This applies
/// no timing policy of its own: the reader decided where each event ends.
pub(crate) fn parse_framed_input(data: &[u8]) -> Vec<RawInputEvent> {
    let mut events = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let Some((event, consumed)) = extract_one_event(rest) else {
            // A lone ESC or Alt introducer the reader released as a key.
            events.extend(events_from_framed_chunks(vec![rest.to_vec()]));
            break;
        };
        events.push(event);
        rest = &rest[consumed..];
    }
    events
}

use crate::input::{parse_terminal_key_sequence, TerminalKey, TextCommit};
use crate::terminal_theme::{
    parse_default_color_response, parse_palette_color_response, DefaultColorKind, HostAppearance,
    RgbColor,
};

const ESC: u8 = 0x1b;
pub(crate) const RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS: i32 = 10;
pub(crate) const MOUSE_ACTIVE_ESCAPE_SEQUENCE_FLUSH_TIMEOUT_MS: i32 = 150;
/// Covers the 33 ms split in #4630 without gluing legacy Alt+[ to the next key.
pub(crate) const MOUSE_ACTIVE_CSI_INTRODUCER_FLUSH_TIMEOUT_MS: i32 = 50;
/// Covers the 350 ms mouse tail delay in #3480. Other input ends it early (#4751).
const DISAMBIGUATED_MOUSE_TAIL_FLUSH_TIMEOUT_MS: i32 = 500;
/// Bytes already inside a sequence cannot be a key, so wait long enough for a
/// split read over a slow link before giving up on them.
pub(crate) const INCOMPLETE_SEQUENCE_FLUSH_TIMEOUT_MS: i32 = 500;
pub(crate) const GHOSTTY_COLOR_SCHEME_DARK_REPORT: &[u8] = b"\x1b[?997;1n";
pub(crate) const GHOSTTY_COLOR_SCHEME_LIGHT_REPORT: &[u8] = b"\x1b[?997;2n";
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug)]
pub enum RawInputEvent {
    Key(TerminalKey),
    Text(TextCommit),
    Paste(String),
    Mouse(MouseEvent),
    OuterFocusGained,
    OuterFocusLost,
    HostDefaultColor {
        kind: DefaultColorKind,
        color: RgbColor,
    },
    HostPaletteColors {
        colors: Vec<(u8, RgbColor)>,
    },
    HostColorSchemeChanged(HostAppearance),
    // The dimensions are only read by the Unix client.
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    HostCellSizeReport {
        width_px: u32,
        height_px: u32,
    },
    Unsupported,
}

/// Events for chunks the byte framer released as complete input.
fn events_from_framed_chunks(chunks: Vec<Vec<u8>>) -> Vec<RawInputEvent> {
    chunks
        .into_iter()
        .filter_map(|chunk| {
            if chunk.as_slice() == [ESC] {
                return Some(RawInputEvent::Key(
                    TerminalKey::new(crossterm::event::KeyCode::Esc, KeyModifiers::empty())
                        .with_vt_bytes(chunk),
                ));
            }
            // The byte framer released this introducer as a key on its own.
            if is_alt_key_introducer(&chunk) {
                let text = std::str::from_utf8(&chunk).ok()?;
                return parse_terminal_key_sequence(text)
                    .map(|key| RawInputEvent::Key(key.with_vt_bytes(chunk.clone())));
            }
            extract_one_event(&chunk).map(|(event, _consumed)| {
                tracing::debug!(raw_bytes = ?chunk, event = ?event, "raw input event parsed");
                event
            })
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct RawInputByteFramer {
    buffer: Vec<u8>,
    discard_until: Option<ControlStringFamily>,
    discarded_tail_bytes: usize,
    // Keep the discarded prefix separate from continuation bytes awaiting validation.
    timed_out_mouse_prefix: Option<Vec<u8>>,
    lone_escape_recently_flushed: bool,
    host_color_replies_awaited: u16,
    host_cell_size_replies_awaited: u16,
    host_appearance_reply_awaited: bool,
    /// Last query sent or reply received. Hosts may answer only part of a
    /// query (or none of it), so the reply counters alone never expire.
    host_reply_activity_at: Option<Instant>,
    /// Last read that left an unfinished OSC 10/11 reply buffered.
    partial_host_color_reply_read_at: Option<Instant>,
    held_pending_host_reply_esc: bool,
    host_color_scheme_change_tracking: bool,
    host_appearance_query_on_focus: bool,
    split_coalesced_escape: bool,
    host_escape_disambiguation_active: bool,
    awaiting_mouse_tail_after: Option<usize>,
}

const HOST_COLOR_QUERY_REPLIES: u16 = 258;
/// How long after the last query or reply Herdr still treats `ESC[` / `ESC]`
/// as a possible host reply instead of a key.
const HOST_REPLY_QUIET_WINDOW: Duration = Duration::from_secs(1);
/// How long an unfinished OSC 10/11 reply may wait for its next read before
/// it is dropped (#5052). Twice the split-read wait for any incomplete
/// sequence, and far above the tens of milliseconds seen between fragments
/// over SSH and tmux, yet short enough that a reply the link cut off cannot
/// hold input for long.
const HOST_COLOR_REPLY_IDLE_LIMIT: Duration = Duration::from_secs(1);
const HOST_CELL_SIZE_QUERY_REPLIES: u16 = 1;
const MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES: usize = 32;

impl RawInputByteFramer {
    pub(crate) fn for_host_input() -> Self {
        Self::with_host_input_policy(
            crate::platform::capabilities().preserve_legacy_doubled_escape_input,
        )
    }

    pub(crate) fn with_host_input_policy(preserve_legacy_doubled_escape_input: bool) -> Self {
        Self {
            split_coalesced_escape: !preserve_legacy_doubled_escape_input,
            ..Self::default()
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        if data.is_empty() {
            return self.push_extended();
        }
        // Bytes after the idle limit are never part of the abandoned reply.
        self.drop_abandoned_host_color_reply();
        self.buffer.extend_from_slice(data);
        let chunks = self.push_extended();
        self.partial_host_color_reply_read_at =
            starts_with_incomplete_default_color_response(&self.buffer).then(Instant::now);
        chunks
    }

    fn push_extended(&mut self) -> Vec<Vec<u8>> {
        if let Some(prefix_len) = self.awaiting_mouse_tail_after.take() {
            if !continues_escape_sequence(&self.buffer) {
                // The prefix already outlived keyboard timing; it was a key.
                let mut chunks = vec![self.buffer.drain(..prefix_len).collect()];
                chunks.extend(self.drain_available_chunks());
                return chunks;
            }
        }
        self.drain_available_chunks()
    }

    /// Hold a lone trailing ESC for one idle flush so an OSC 10/11 reply split
    /// at its ESC introducer stitches back together instead of leaking (#549).
    /// Overlapping queries each add their own replies to the window.
    pub(crate) fn host_color_query_sent(&mut self) {
        self.host_color_query_sent_with_replies(HOST_COLOR_QUERY_REPLIES);
    }

    fn host_color_query_sent_with_replies(&mut self, replies: u16) {
        self.host_color_replies_awaited = self.host_color_replies_awaited.saturating_add(replies);
        self.held_pending_host_reply_esc = false;
        self.host_reply_activity_at = Some(Instant::now());
    }

    /// Whether a buffered unfinished OSC 10/11 reply has waited past its idle
    /// limit or grown past any real reply, so it can no longer hold input.
    fn host_color_reply_abandoned(&self) -> bool {
        // The Windows default color query keeps its own reply deadline.
        self.buffer.len() >= MAX_DISCARDED_CONTROL_TAIL_BYTES
            || self
                .partial_host_color_reply_read_at
                .is_none_or(|at| at.elapsed() >= HOST_COLOR_REPLY_IDLE_LIMIT)
    }

    fn drop_abandoned_host_color_reply(&mut self) {
        if self.partial_host_color_reply_read_at.is_none()
            || !starts_with_incomplete_default_color_response(&self.buffer)
            || !self.host_color_reply_abandoned()
        {
            return;
        }
        tracing::debug!(
            len = self.buffer.len(),
            "dropping unfinished host color response before new input"
        );
        self.buffer.clear();
        self.partial_host_color_reply_read_at = None;
    }

    fn host_appearance_query_sent(&mut self) {
        self.host_appearance_reply_awaited = true;
        self.held_pending_host_reply_esc = false;
        self.host_reply_activity_at = Some(Instant::now());
    }

    /// Same hold window as `host_color_query_sent`, for the XTWINOPS cell size
    /// reply. Only the Unix client sends this query.
    pub(crate) fn host_cell_size_query_sent(&mut self) {
        self.host_cell_size_replies_awaited = HOST_CELL_SIZE_QUERY_REPLIES;
        self.held_pending_host_reply_esc = false;
        self.host_reply_activity_at = Some(Instant::now());
    }

    fn awaiting_host_reply(&self) -> bool {
        (self.host_color_replies_awaited > 0
            || self.host_cell_size_replies_awaited > 0
            || self.host_appearance_reply_awaited)
            && self
                .host_reply_activity_at
                .is_some_and(|at| at.elapsed() < HOST_REPLY_QUIET_WINDOW)
    }

    /// Stop expecting replies the host never sent, so their reply handling no
    /// longer holds or discards what is now ordinary input.
    fn expire_unanswered_host_queries(&mut self) {
        if self.awaiting_host_reply() || self.host_reply_activity_at.is_none() {
            return;
        }
        // The Windows default color query keeps its own reply deadline.
        self.host_color_replies_awaited = 0;
        self.host_cell_size_replies_awaited = 0;
        self.host_appearance_reply_awaited = false;
        self.held_pending_host_reply_esc = false;
        self.host_reply_activity_at = None;
    }

    pub(crate) fn enable_host_color_scheme_change_tracking(&mut self) {
        self.host_color_scheme_change_tracking = true;
    }

    /// Arm the bounded host-reply window when focus gain will emit an appearance query.
    /// If the write or reply fails, a lone Escape is delayed for only one extra flush.
    pub(crate) fn enable_host_appearance_query_on_focus(&mut self) {
        self.host_appearance_query_on_focus = true;
    }

    pub(crate) fn has_pending_input(&self) -> bool {
        !self.buffer.is_empty()
    }

    pub(crate) fn set_host_escape_disambiguation_active(&mut self, active: bool) {
        self.host_escape_disambiguation_active = active;
    }

    /// Pending bytes that could still be a key on their own: a lone ESC or ESC
    /// plus one introducer. Legacy hosts send those for Escape and Alt+key, and
    /// host text bindings (e.g. Ghostty `text:\x1bO`) send them even when the
    /// host disambiguates escapes.
    fn pending_could_be_escape_key(&self) -> bool {
        is_escape_key_prefix(&self.buffer)
    }

    /// How long to wait for more input before `flush_timeout`. Only bytes that
    /// could still be a key get the short keyboard window; anything already
    /// inside a sequence waits long enough for split reads over slow links.
    pub(crate) fn idle_flush_timeout_ms(&self, host_mouse_capture_active: bool) -> i32 {
        if self.buffer.is_empty() {
            return RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS;
        }
        // A reply Herdr asked for can split right after its introducer.
        if self.awaiting_host_reply() && matches!(self.buffer.as_slice(), b"\x1b[" | b"\x1b]") {
            return INCOMPLETE_SEQUENCE_FLUSH_TIMEOUT_MS;
        }
        if self.pending_could_be_escape_key() {
            if !host_mouse_capture_active {
                return RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS;
            }
            return match self.buffer.as_slice() {
                [ESC] => MOUSE_ACTIVE_ESCAPE_SEQUENCE_FLUSH_TIMEOUT_MS,
                // A mouse report split after ESC[ is still ambiguous with legacy Alt+[.
                b"\x1b[" => MOUSE_ACTIVE_CSI_INTRODUCER_FLUSH_TIMEOUT_MS,
                _ => RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS,
            };
        }
        if self.buffer.first() == Some(&ESC) || starts_with_incomplete_utf8_char(&self.buffer) {
            return INCOMPLETE_SEQUENCE_FLUSH_TIMEOUT_MS;
        }
        RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS
    }

    /// How long to keep holding input after a first idle flush held it.
    pub(crate) fn held_input_flush_timeout_ms(&self) -> i32 {
        if self.awaiting_mouse_tail_after.is_some() {
            DISAMBIGUATED_MOUSE_TAIL_FLUSH_TIMEOUT_MS
        } else {
            RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS
        }
    }

    pub(crate) fn flush_timeout(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = self.drain_available_chunks();
        self.expire_unanswered_host_queries();

        // Idle is not evidence that a mouse report has ended. The continuation
        // stays bounded and is released if it cannot complete a valid report.
        if self.timed_out_mouse_prefix.is_some() {
            return chunks;
        }

        if let Some(family) = self.discard_until {
            if family == ControlStringFamily::HostReplyCsi {
                return chunks;
            }
            let keep_split_st = self.buffer.last() == Some(&ESC);
            let keep_discarding = plausible_control_string_tail(family, &self.buffer);
            self.discarded_tail_bytes = self.discarded_tail_bytes.saturating_add(self.buffer.len());
            self.buffer.clear();
            if keep_discarding && self.discarded_tail_bytes <= MAX_DISCARDED_CONTROL_TAIL_BYTES {
                if keep_split_st {
                    self.buffer.push(ESC);
                }
            } else {
                self.discard_until = None;
                self.discarded_tail_bytes = 0;
            }
            return chunks;
        }

        if self.buffer.is_empty() {
            return chunks;
        }

        // A confirmed host sends Escape as `CSI 27u`, so a mouse report prefix
        // that outlives keyboard timing may still get a delayed tail (#3480).
        // Keep it briefly; `push` releases it if other input follows.
        // A finished mouse wait also counts as this prefix's one-flush host
        // reply hold, so the two holds never stack.
        let mouse_wait_served = self.awaiting_mouse_tail_after.take().is_some();
        // Prefixes already inside a sequence waited the long window; only the
        // ambiguous ESC / ESC[ prefixes need this extra mouse-tail hold.
        if !mouse_wait_served
            && self.host_escape_disambiguation_active
            && self.pending_could_be_escape_key()
            && could_continue_as_mouse_report(&self.buffer)
        {
            self.awaiting_mouse_tail_after = Some(self.buffer.len());
            return chunks;
        }

        if self.lone_escape_recently_flushed && self.buffer.starts_with(b"[<") {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete orphaned SGR mouse tail after input timeout"
            );
            let mut prefix = vec![ESC];
            prefix.append(&mut self.buffer);
            self.retain_timed_out_mouse_prefix(prefix);
            self.lone_escape_recently_flushed = false;
            return chunks;
        }

        if starts_with_incomplete_sgr_mouse_sequence(&self.buffer) {
            tracing::debug!(
                bytes = ?self.buffer,
                "discarding incomplete SGR mouse sequence after input timeout"
            );
            let prefix = std::mem::take(&mut self.buffer);
            self.retain_timed_out_mouse_prefix(prefix);
            return chunks;
        }

        if self.buffer.starts_with(BRACKETED_PASTE_START)
            && find_subsequence(&self.buffer, BRACKETED_PASTE_END).is_none()
        {
            tracing::trace!(
                len = self.buffer.len(),
                "waiting for bracketed paste terminator"
            );
            return chunks;
        }

        if starts_with_incomplete_default_color_response(&self.buffer) {
            if self.host_color_reply_abandoned() {
                // The host went quiet mid-reply; never let it hold input (#5052).
                tracing::debug!(
                    len = self.buffer.len(),
                    "dropping unfinished host color response after idle limit"
                );
                self.buffer.clear();
                self.partial_host_color_reply_read_at = None;
                return chunks;
            }
            tracing::trace!(
                len = self.buffer.len(),
                "waiting for host color response terminator"
            );
            return chunks;
        }

        if (self.host_cell_size_replies_awaited > 0 || self.host_appearance_reply_awaited)
            && self.buffer.as_slice() == b"\x1b["
        {
            if !self.held_pending_host_reply_esc && !mouse_wait_served {
                self.held_pending_host_reply_esc = true;
                tracing::trace!("holding incomplete host CSI reply one flush");
                return chunks;
            }
            self.host_cell_size_replies_awaited = 0;
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
        }

        if self.host_cell_size_replies_awaited > 0
            && starts_with_incomplete_host_cell_size_report(&self.buffer)
        {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host cell size report after input timeout"
            );
            self.host_cell_size_replies_awaited = 0;
            self.held_pending_host_reply_esc = false;
            self.discard_until = Some(ControlStringFamily::HostReplyCsi);
            self.discarded_tail_bytes = 0;
            self.buffer.clear();
            return chunks;
        }

        if starts_with_incomplete_host_color_scheme_report(&self.buffer) {
            if self.host_appearance_reply_awaited && !self.held_pending_host_reply_esc {
                self.held_pending_host_reply_esc = true;
                tracing::trace!(
                    len = self.buffer.len(),
                    "holding incomplete host color scheme report one flush"
                );
                return chunks;
            }
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host color scheme report after input timeout"
            );
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
            self.discard_until = Some(ControlStringFamily::HostReplyCsi);
            self.discarded_tail_bytes = 0;
            self.buffer.clear();
            return chunks;
        }

        // A legacy host sends Alt+[, Alt+O and Alt+Escape as ESC plus that byte. When nothing
        // followed and no reply is expected, it was the key. String introducers
        // (OSC, DCS, APC, PM, SOS) also start host replies, so they keep the
        // control-string path and are never forwarded as Alt keys (#344).
        if is_alt_key_introducer(&self.buffer) && !self.awaiting_host_reply() {
            chunks.push(std::mem::take(&mut self.buffer));
            return chunks;
        }

        if let Some(ControlString::Incomplete { family }) = control_string(&self.buffer) {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host control string after input timeout"
            );
            // This intentionally gives host control replies precedence over legacy
            // Alt forms like Alt+] after timeout, so later reply tails cannot leak.
            self.discard_until = Some(family);
            self.discarded_tail_bytes = 0;
            self.buffer.clear();
            return chunks;
        }

        if self.buffer.as_slice() == [ESC] {
            if self.awaiting_host_reply() && !self.held_pending_host_reply_esc && !mouse_wait_served
            {
                self.held_pending_host_reply_esc = true;
                tracing::trace!("holding lone escape one flush while awaiting host reply");
                return chunks;
            }
            // No continuation arrived; give up the window so Escape is not delayed again.
            self.host_color_replies_awaited = 0;
            self.host_cell_size_replies_awaited = 0;
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
            tracing::warn!(
                bytes = ?self.buffer,
                "flushing lone escape after input timeout; if this follows an alt chord or focus switch it may reach the pane as plain esc"
            );
            self.lone_escape_recently_flushed = true;
            chunks.push(std::mem::take(&mut self.buffer));
            return chunks;
        }

        if let Ok(text) = std::str::from_utf8(&self.buffer) {
            if parse_terminal_key_sequence(text).is_some() {
                chunks.push(std::mem::take(&mut self.buffer));
                return chunks;
            }
        }

        if starts_with_incomplete_utf8_char(&self.buffer) {
            tracing::trace!(bytes = ?self.buffer, "waiting for UTF-8 continuation bytes");
            return chunks;
        }

        if self.buffer.first() == Some(&ESC) && starts_with_incomplete_utf8_char(&self.buffer[1..])
        {
            tracing::trace!(bytes = ?self.buffer, "waiting for escaped UTF-8 continuation bytes");
            return chunks;
        }

        tracing::debug!(bytes = ?self.buffer, "dropping incomplete raw input buffer after timeout");
        self.lone_escape_recently_flushed = false;
        self.buffer.clear();
        chunks
    }

    fn retain_timed_out_mouse_prefix(&mut self, prefix: Vec<u8>) {
        self.timed_out_mouse_prefix = (prefix.len() < MAX_DISCARDED_CONTROL_TAIL_BYTES
            && plausible_sgr_mouse_prefix(&prefix))
        .then_some(prefix);
    }

    fn drain_available_chunks(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();

        loop {
            if let Some(prefix) = &self.timed_out_mouse_prefix {
                match classify_sgr_mouse_continuation(prefix, &self.buffer) {
                    SgrMouseContinuation::Incomplete => break,
                    SgrMouseContinuation::Complete(len) => {
                        self.buffer.drain(..len);
                    }
                    SgrMouseContinuation::Invalid => {}
                }
                self.timed_out_mouse_prefix = None;
            }

            if self.lone_escape_recently_flushed {
                if starts_with_incomplete_orphaned_sgr_mouse_tail(&self.buffer) {
                    break;
                }
                if discard_complete_orphaned_sgr_mouse_tail(&mut self.buffer) {
                    self.lone_escape_recently_flushed = false;
                    continue;
                }
                self.lone_escape_recently_flushed = false;
            }

            if let Some(family) = self.discard_until {
                if family == ControlStringFamily::HostReplyCsi {
                    if discard_host_reply_csi_tail(&mut self.buffer, &mut self.discarded_tail_bytes)
                    {
                        self.discard_until = None;
                        self.discarded_tail_bytes = 0;
                        continue;
                    }
                    break;
                }
                let Some(terminator_len) =
                    control_string_terminator_for_family(&self.buffer, family)
                else {
                    break;
                };
                self.buffer.drain(..terminator_len);
                self.discard_until = None;
                self.discarded_tail_bytes = 0;
                continue;
            }

            // Kitty key reports carry Alt in their modifiers and are never
            // Alt-prefixed, so ESC before a complete report is its own key. WezTerm
            // sends an Escape press as bare ESC and the release as a report,
            // and a quick tap delivers both together (#1266).
            if self.buffer.starts_with(b"\x1b\x1b")
                && (self.split_coalesced_escape || starts_with_kitty_key_report(&self.buffer[1..]))
            {
                chunks.push(vec![ESC]);
                self.buffer.drain(..1);
                continue;
            }

            let Some((event, consumed)) = extract_one_event(&self.buffer) else {
                break;
            };
            if matches!(
                event,
                RawInputEvent::HostDefaultColor { .. } | RawInputEvent::HostPaletteColors { .. }
            ) {
                self.host_color_replies_awaited = self.host_color_replies_awaited.saturating_sub(1);
            } else if matches!(event, RawInputEvent::HostCellSizeReport { .. }) {
                self.host_cell_size_replies_awaited =
                    self.host_cell_size_replies_awaited.saturating_sub(1);
            } else if self.host_appearance_query_on_focus
                && matches!(event, RawInputEvent::OuterFocusGained)
            {
                self.host_appearance_query_sent();
            } else if matches!(event, RawInputEvent::HostColorSchemeChanged(_)) {
                self.host_appearance_reply_awaited = false;
                if self.host_color_scheme_change_tracking {
                    self.host_color_query_sent();
                }
            }
            if is_host_reply_event(&event) && self.host_reply_activity_at.is_some() {
                // Long reply bursts (256 palette entries) stay protected.
                self.host_reply_activity_at = Some(Instant::now());
            }
            self.held_pending_host_reply_esc = false;
            chunks.push(self.buffer[..consumed].to_vec());
            self.buffer.drain(..consumed);
        }

        chunks
    }
}

const MAX_DISCARDED_CONTROL_TAIL_BYTES: usize = 128;

/// Whether `bytes` starts with a complete sequence shaped like a kitty key
/// report, `CSI <code>[:alternates][;mods[:event][;text]] u`. Only the shape is
/// checked: digit-led parameters made of digits, `;` and `:`, ending in `u`.
/// That excludes `CSI u` (cursor restore) and private replies like `CSI ? 7 u`.
fn starts_with_kitty_key_report(bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_prefix(b"\x1b[") else {
        return false;
    };
    let params = rest
        .iter()
        .take_while(|byte| byte.is_ascii_digit() || matches!(byte, b';' | b':'))
        .count();
    rest.first().is_some_and(u8::is_ascii_digit) && rest.get(params) == Some(&b'u')
}

fn plausible_control_string_tail(family: ControlStringFamily, buffer: &[u8]) -> bool {
    match family {
        ControlStringFamily::Osc => buffer.iter().all(|byte| {
            byte.is_ascii_digit()
                || matches!(
                    *byte,
                    b';' | b':'
                        | b'/'
                        | b'#'
                        | b'?'
                        | b'.'
                        | b'_'
                        | b'-'
                        | b'+'
                        | b'r'
                        | b'g'
                        | b'b'
                        | b'R'
                        | b'G'
                        | b'B'
                        | ESC
                )
        }),
        ControlStringFamily::StTerminated => buffer.last() == Some(&ESC),
        ControlStringFamily::HostReplyCsi => false,
    }
}

pub(crate) fn events_require_host_mode_refresh(events: &[RawInputEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RawInputEvent::OuterFocusGained))
}

fn extract_one_event(buffer: &[u8]) -> Option<(RawInputEvent, usize)> {
    if buffer.is_empty() {
        return None;
    }

    if buffer.starts_with(BRACKETED_PASTE_START) {
        let end = find_subsequence(buffer, BRACKETED_PASTE_END)?;
        let content = std::str::from_utf8(&buffer[BRACKETED_PASTE_START.len()..end]).ok()?;
        return Some((
            RawInputEvent::Paste(content.to_string()),
            end + BRACKETED_PASTE_END.len(),
        ));
    }

    if buffer[0] == ESC {
        let seq_len = complete_escape_sequence_len(buffer)?;
        if buffer[..seq_len].starts_with(b"\x1b[M") {
            let event = parse_default_mouse(&buffer[..seq_len])
                .map(RawInputEvent::Mouse)
                .unwrap_or(RawInputEvent::Unsupported);
            return Some((event, seq_len));
        }
        let seq = std::str::from_utf8(&buffer[..seq_len]).ok()?;

        if let Some((kind, color)) = parse_default_color_response(seq) {
            return Some((RawInputEvent::HostDefaultColor { kind, color }, seq_len));
        }
        if let Some((index, color)) = parse_palette_color_response(seq) {
            return Some((
                RawInputEvent::HostPaletteColors {
                    colors: vec![(index, color)],
                },
                seq_len,
            ));
        }

        match seq {
            "\x1b[I" => return Some((RawInputEvent::OuterFocusGained, seq_len)),
            "\x1b[O" => return Some((RawInputEvent::OuterFocusLost, seq_len)),
            _ => {}
        }

        if let Some(appearance) = parse_host_color_scheme_report(&buffer[..seq_len]) {
            return Some((RawInputEvent::HostColorSchemeChanged(appearance), seq_len));
        }

        if let Some((width_px, height_px)) = parse_host_cell_size_report(&buffer[..seq_len]) {
            return Some((
                RawInputEvent::HostCellSizeReport {
                    width_px,
                    height_px,
                },
                seq_len,
            ));
        }

        if let Some(mouse) = parse_sgr_mouse(seq) {
            return Some((RawInputEvent::Mouse(mouse), seq_len));
        }

        if let Some(key) = parse_terminal_key_sequence(seq) {
            return Some((
                RawInputEvent::Key(key.with_vt_bytes(buffer[..seq_len].to_vec())),
                seq_len,
            ));
        }

        tracing::debug!(sequence = ?seq, "dropping unsupported escape sequence");
        return Some((RawInputEvent::Unsupported, seq_len));
    }

    let consumed = first_complete_utf8_char_len(buffer)?;
    let text = std::str::from_utf8(&buffer[..consumed]).ok()?;
    let key = parse_terminal_key_sequence(text)?
        .with_text_commit()
        .with_vt_bytes(buffer[..consumed].to_vec());
    Some((RawInputEvent::Key(key), consumed))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlStringFamily {
    Osc,
    StTerminated,
    HostReplyCsi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlString {
    Complete {
        len: usize,
        family: ControlStringFamily,
    },
    Incomplete {
        family: ControlStringFamily,
    },
}

fn parse_host_color_scheme_report(buffer: &[u8]) -> Option<HostAppearance> {
    match buffer {
        GHOSTTY_COLOR_SCHEME_DARK_REPORT => Some(HostAppearance::Dark),
        GHOSTTY_COLOR_SCHEME_LIGHT_REPORT => Some(HostAppearance::Light),
        _ => None,
    }
}

/// Parses an XTWINOPS cell size report (`CSI 6 ; height ; width t`) into
/// `(width_px, height_px)`; note the reply orders height first.
fn parse_host_cell_size_report(buffer: &[u8]) -> Option<(u32, u32)> {
    let body = buffer.strip_prefix(b"\x1b[")?.strip_suffix(b"t")?;
    let text = std::str::from_utf8(body).ok()?;
    let mut params = text.split(';');
    if params.next()? != "6" {
        return None;
    }
    let height_px = params.next()?.parse::<u32>().ok()?;
    let width_px = params.next()?.parse::<u32>().ok()?;
    if params.next().is_some() || width_px == 0 || height_px == 0 {
        return None;
    }
    Some((width_px, height_px))
}

fn starts_with_incomplete_default_color_response(buffer: &[u8]) -> bool {
    // The prefix check keeps this constant-cost for ordinary input.
    matches!(buffer.get(..5), Some(b"\x1b]10;" | b"\x1b]11;"))
        && matches!(
            control_string(buffer),
            Some(ControlString::Incomplete {
                family: ControlStringFamily::Osc
            })
        )
}

fn starts_with_incomplete_host_color_scheme_report(buffer: &[u8]) -> bool {
    buffer.starts_with(b"\x1b[?")
        && (GHOSTTY_COLOR_SCHEME_DARK_REPORT.starts_with(buffer)
            || GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.starts_with(buffer))
        && buffer.len() < GHOSTTY_COLOR_SCHEME_DARK_REPORT.len()
}

fn starts_with_incomplete_host_cell_size_report(buffer: &[u8]) -> bool {
    let Some(body) = buffer.strip_prefix(b"\x1b[") else {
        return false;
    };
    if body.is_empty() || body.last() == Some(&b't') {
        return false;
    }

    let mut params = body.split(|byte| *byte == b';');
    if params.next() != Some(b"6".as_slice()) {
        return false;
    }
    let height = params.next();
    let width = params.next();
    params.next().is_none()
        && height.is_none_or(|value| value.iter().all(u8::is_ascii_digit))
        && width.is_none_or(|value| value.iter().all(u8::is_ascii_digit))
        && !(height.is_some_and(<[u8]>::is_empty) && width.is_some())
}

fn is_host_reply_event(event: &RawInputEvent) -> bool {
    matches!(
        event,
        RawInputEvent::HostDefaultColor { .. }
            | RawInputEvent::HostPaletteColors { .. }
            | RawInputEvent::HostCellSizeReport { .. }
            | RawInputEvent::HostColorSchemeChanged(_)
    )
}

/// ESC plus a byte that can also start a longer sequence: Alt+[, Alt+O, and
/// Alt+Escape (ESC ESC, kept whole where doubled escapes are preserved).
fn is_alt_key_introducer(buffer: &[u8]) -> bool {
    matches!(buffer, [ESC, b'[' | b'O' | ESC])
}

fn is_escape_key_prefix(buffer: &[u8]) -> bool {
    match buffer {
        [ESC] => true,
        [ESC, introducer] => matches!(
            introducer,
            b'[' | b'O' | b'P' | b']' | b'X' | b'^' | b'_' | 0x1b
        ),
        _ => false,
    }
}

fn control_string(buffer: &[u8]) -> Option<ControlString> {
    let family = match buffer.get(..2)? {
        b"\x1b]" => ControlStringFamily::Osc,
        b"\x1bP" | b"\x1b_" | b"\x1b^" | b"\x1bX" => ControlStringFamily::StTerminated,
        _ => return None,
    };

    Some(match control_string_terminator_for_family(buffer, family) {
        Some(len) => ControlString::Complete { len, family },
        None => ControlString::Incomplete { family },
    })
}

fn first_complete_utf8_char_len(buffer: &[u8]) -> Option<usize> {
    let width = utf8_char_width(*buffer.first()?)?;

    if buffer.len() < width {
        return None;
    }

    std::str::from_utf8(&buffer[..width]).ok()?;
    Some(width)
}

fn starts_with_incomplete_utf8_char(buffer: &[u8]) -> bool {
    match std::str::from_utf8(buffer) {
        Ok(_) => false,
        Err(err) => err.valid_up_to() == 0 && err.error_len().is_none(),
    }
}

fn utf8_char_width(first: u8) -> Option<usize> {
    if first < 0x80 {
        Some(1)
    } else if first & 0b1110_0000 == 0b1100_0000 {
        Some(2)
    } else if first & 0b1111_0000 == 0b1110_0000 {
        Some(3)
    } else if first & 0b1111_1000 == 0b1111_0000 {
        Some(4)
    } else {
        None
    }
}

fn complete_escape_sequence_len(buffer: &[u8]) -> Option<usize> {
    if buffer.len() == 1 {
        return None;
    }

    if buffer.starts_with(b"\x1b\x1b[<") {
        if let Some(mouse_len) = find_csi_final(&buffer[1..], b"Mm") {
            let mouse_sequence = std::str::from_utf8(&buffer[1..1 + mouse_len]).ok()?;
            if parse_sgr_mouse(mouse_sequence).is_some() {
                return Some(1);
            }
        }
    }

    if buffer.len() >= 7
        && buffer.starts_with(b"\x1b\x1b[M")
        && parse_default_mouse(&buffer[1..7]).is_some()
    {
        return Some(1);
    }

    if buffer.starts_with(b"\x1b\x1b") {
        return complete_escape_sequence_len(&buffer[1..]).map(|len| len + 1);
    }

    if buffer.starts_with(b"\x1b[") {
        if buffer.starts_with(b"\x1b[<") {
            return find_csi_final(buffer, b"Mm");
        }
        if buffer.starts_with(b"\x1b[M") {
            return (buffer.len() >= 6).then_some(6);
        }
        return find_csi_final(
            buffer,
            b"@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~",
        );
    }

    if let Some(control) = control_string(buffer) {
        return match control {
            ControlString::Complete { len, .. } => Some(len),
            ControlString::Incomplete { .. } => None,
        };
    }

    if buffer.starts_with(b"\x1bO") {
        return (buffer.len() >= 3).then_some(3);
    }

    let escaped_char_width = utf8_char_width(buffer[1])?;
    if buffer.len() < 1 + escaped_char_width {
        return None;
    }
    std::str::from_utf8(&buffer[1..1 + escaped_char_width]).ok()?;
    Some(1 + escaped_char_width)
}

fn starts_with_incomplete_sgr_mouse_sequence(buffer: &[u8]) -> bool {
    buffer.starts_with(b"\x1b[<")
        && buffer[3..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

/// Whether bytes after a held ESC or ESC[ still read as one escape sequence,
/// such as a mouse report, host reply, or bracketed paste, rather than a key.
fn continues_escape_sequence(buffer: &[u8]) -> bool {
    match buffer {
        [ESC] | [ESC, b'['] => true,
        // OSC, DCS and APC host replies can also split after their ESC.
        [ESC, b']' | b'P' | b'_', ..] => true,
        [ESC, b'[', next, ..] => *next == b'M' || matches!(*next, 0x20..=0x3f),
        _ => false,
    }
}

fn could_continue_as_mouse_report(buffer: &[u8]) -> bool {
    [b"\x1b[<".as_slice(), b"\x1b[M"]
        .iter()
        .any(|prefix| buffer.starts_with(prefix) || prefix.starts_with(buffer))
}

fn starts_with_incomplete_orphaned_sgr_mouse_tail(buffer: &[u8]) -> bool {
    if buffer.len() > MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES {
        return false;
    }
    buffer.len() < 3 && b"[<".starts_with(buffer)
        || buffer.starts_with(b"[<")
            && buffer[2..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

fn discard_complete_orphaned_sgr_mouse_tail(buffer: &mut Vec<u8>) -> bool {
    let Some(terminator_len) = buffer
        .iter()
        .position(|byte| matches!(*byte, b'M' | b'm'))
        .map(|idx| idx + 1)
    else {
        return false;
    };
    if terminator_len > MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES {
        return false;
    }
    let mut sequence = Vec::with_capacity(terminator_len + 1);
    sequence.push(ESC);
    sequence.extend_from_slice(&buffer[..terminator_len]);
    let Ok(sequence) = std::str::from_utf8(&sequence) else {
        return false;
    };
    if parse_sgr_mouse(sequence).is_none() {
        return false;
    }
    buffer.drain(..terminator_len);
    true
}

fn discard_host_reply_csi_tail(buffer: &mut Vec<u8>, discarded_tail_bytes: &mut usize) -> bool {
    let remaining = MAX_DISCARDED_CONTROL_TAIL_BYTES.saturating_sub(*discarded_tail_bytes);
    let inspected = buffer.len().min(remaining);

    for index in 0..inspected {
        match buffer[index] {
            0x20..=0x3f => {}
            0x40..=0x7e => {
                buffer.drain(..=index);
                return true;
            }
            _ => {
                buffer.drain(..index);
                return true;
            }
        }
    }

    buffer.drain(..inspected);
    *discarded_tail_bytes = discarded_tail_bytes.saturating_add(inspected);
    *discarded_tail_bytes >= MAX_DISCARDED_CONTROL_TAIL_BYTES
}

enum SgrMouseContinuation {
    Incomplete,
    Complete(usize),
    Invalid,
}

fn classify_sgr_mouse_continuation(prefix: &[u8], tail: &[u8]) -> SgrMouseContinuation {
    let remaining = MAX_DISCARDED_CONTROL_TAIL_BYTES.saturating_sub(prefix.len());
    let tail = &tail[..tail.len().min(remaining)];
    let final_index = tail
        .iter()
        .position(|byte| !byte.is_ascii_digit() && *byte != b';');
    let payload_len = final_index.unwrap_or(tail.len());
    let mut report = prefix.to_vec();
    report.extend_from_slice(&tail[..payload_len]);
    if !plausible_sgr_mouse_prefix(&report) {
        return SgrMouseContinuation::Invalid;
    }
    if let Some(index) = final_index {
        report.push(tail[index]);
        let valid = std::str::from_utf8(&report)
            .ok()
            .and_then(parse_sgr_mouse)
            .is_some();
        return if valid {
            SgrMouseContinuation::Complete(index + 1)
        } else {
            SgrMouseContinuation::Invalid
        };
    }
    if report.len() >= MAX_DISCARDED_CONTROL_TAIL_BYTES {
        SgrMouseContinuation::Invalid
    } else {
        SgrMouseContinuation::Incomplete
    }
}

// Reject impossible continuations early, without changing the general mouse
// parser. A partial last field (including zero) can still become valid.
fn plausible_sgr_mouse_prefix(report: &[u8]) -> bool {
    let Some(body) = report.strip_prefix(b"\x1b[<") else {
        return false;
    };
    let mut fields = body.split(|byte| *byte == b';').enumerate().peekable();
    while let Some((field, digits)) = fields.next() {
        if field > 2 {
            return false;
        }
        if digits.is_empty() {
            return fields.peek().is_none();
        }
        if !digits.iter().all(u8::is_ascii_digit) {
            return false;
        }
        let Some(value) = std::str::from_utf8(digits)
            .ok()
            .and_then(|digits| digits.parse::<u16>().ok())
        else {
            return false;
        };
        if field == 0 && value > u16::from(u8::MAX) {
            return false;
        }
        if fields.peek().is_some()
            && ((field == 0 && parse_mouse_cb(value as u8).is_none()) || (field == 1 && value == 0))
        {
            return false;
        }
    }
    true
}

fn osc_string_terminator(buffer: &[u8]) -> Option<usize> {
    let st = find_subsequence(buffer, b"\x1b\\").map(|idx| idx + 2);
    let bel = buffer
        .iter()
        .position(|byte| *byte == b'\x07')
        .map(|idx| idx + 1);

    match (st, bel) {
        (Some(st), Some(bel)) => Some(st.min(bel)),
        (Some(st), None) => Some(st),
        (None, Some(bel)) => Some(bel),
        (None, None) => None,
    }
}

fn st_string_terminator(buffer: &[u8]) -> Option<usize> {
    find_subsequence(buffer, b"\x1b\\").map(|idx| idx + 2)
}

fn control_string_terminator_for_family(
    buffer: &[u8],
    family: ControlStringFamily,
) -> Option<usize> {
    match family {
        ControlStringFamily::Osc => osc_string_terminator(buffer),
        ControlStringFamily::StTerminated => st_string_terminator(buffer),
        ControlStringFamily::HostReplyCsi => None,
    }
}

fn find_csi_final(buffer: &[u8], finals: &[u8]) -> Option<usize> {
    for (idx, byte) in buffer.iter().enumerate().skip(2) {
        if finals.contains(byte) {
            return Some(idx + 1);
        }
    }
    None
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_default_mouse(sequence: &[u8]) -> Option<MouseEvent> {
    let &[ESC, b'[', b'M', encoded_cb, encoded_column, encoded_row] = sequence else {
        return None;
    };
    let cb = encoded_cb.checked_sub(32)?;
    let column = u16::from(encoded_column).checked_sub(33)?;
    let row = u16::from(encoded_row).checked_sub(33)?;
    let (kind, modifiers) = parse_mouse_cb(cb)?;

    Some(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

fn parse_sgr_mouse(sequence: &str) -> Option<MouseEvent> {
    let report = parse_sgr_mouse_report(sequence.as_bytes())?;
    let column = u16::try_from(report.x.checked_sub(1)?).ok()?;
    let row = u16::try_from(report.y.checked_sub(1)?).ok()?;
    Some(report.at_cell(column, row))
}

/// One SGR mouse report (`CSI < Cb ; x ; y M/m`) with its raw 1-based
/// coordinates: cells normally, pixels when the host reports SGR pixels (1016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SgrMouseReport {
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    pub(crate) x: u32,
    pub(crate) y: u32,
}

impl SgrMouseReport {
    /// The report as a mouse event at a 0-based cell.
    pub(crate) fn at_cell(self, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: self.kind,
            column,
            row,
            modifiers: self.modifiers,
        }
    }
}

pub(crate) fn parse_sgr_mouse_report(sequence: &[u8]) -> Option<SgrMouseReport> {
    let body = sequence.strip_prefix(b"\x1b[<")?;
    let (&final_byte, payload) = body.split_last()?;
    if final_byte != b'M' && final_byte != b'm' {
        return None;
    }
    let mut parts = payload.split(|byte| *byte == b';');
    let cb = u8::try_from(parse_report_number(parts.next()?)?).ok()?;
    let x = parse_report_number(parts.next()?)?;
    let y = parse_report_number(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    let (kind, modifiers) = parse_mouse_cb(cb)?;
    let kind = match (final_byte, kind) {
        (b'm', MouseEventKind::Down(button)) => MouseEventKind::Up(button),
        (_, kind) => kind,
    };
    Some(SgrMouseReport {
        kind,
        modifiers,
        x,
        y,
    })
}

fn parse_report_number(value: &[u8]) -> Option<u32> {
    (!value.is_empty() && value.iter().all(u8::is_ascii_digit))
        .then(|| std::str::from_utf8(value).ok()?.parse().ok())
        .flatten()
}

fn parse_mouse_cb(cb: u8) -> Option<(MouseEventKind, KeyModifiers)> {
    let button_number = (cb & 0b0000_0011) | ((cb & 0b1100_0000) >> 4);
    let dragging = cb & 0b0010_0000 == 0b0010_0000;

    let kind = match (button_number, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        // Crossterm cannot represent extended-button drags. Preserve their
        // position as motion so a stuck host button cannot suppress hover.
        (3, true) | (4, true) | (5, true) | (8, true) | (9, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return None,
    };

    let mut modifiers = KeyModifiers::empty();
    if cb & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }

    Some((kind, modifiers))
}

#[cfg(test)]
#[path = "raw_input_tests.rs"]
mod tests;

#[cfg(test)]
pub use tests::parse_raw_input_bytes_sync;
#[cfg(test)]
pub(crate) use tests::{events_require_host_terminal_theme_query, RawInputFramer};
