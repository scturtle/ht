//! Live PTY stream processing, synchronized-output timeout and terminal effects.
//! Keep tracker/write/reply order and the core -> key-encoder lock order intact.

use std::time::Duration;

use tracing::{debug, error};

use super::{GhosttyPaneTerminal, ProcessBytesResult};
use crate::layout::PaneId;
use crate::pane::input::configure_key_encoder;
use crate::pane::osc::parse_reported_cwd;

pub(super) const KITTY_GRAPHICS_REDRAW_SETTLE: Duration = Duration::from_millis(20);

impl GhosttyPaneTerminal {
    pub fn process_pty_bytes(&self, pane_id: PaneId, bytes: &[u8]) -> ProcessBytesResult {
        crate::render_prof::counter("pty.bytes", bytes.len() as u64);
        let Ok(mut core) = self.core.lock() else {
            error!(pane = pane_id.raw(), "ghostty core lock poisoned in reader");
            return ProcessBytesResult::default();
        };

        discard_terminal_effects(&mut core.terminal);

        core.osc_debug_tracker.observe(bytes);
        for event in core.osc_debug_tracker.drain_pending() {
            debug!(
                pane = pane_id.raw(),
                osc_command = %event.command,
                osc_payload = ?event.payload,
                "agent OSC evidence observed"
            );
        }
        let terminal_title_changed = core.agent_osc_state.observe(bytes);

        core.kitty_keyboard.observe(bytes);
        let mut terminal_responses = Vec::new();
        core.default_color_event_tracker.observe(bytes);
        core.c1_xtgettcap_tracker.observe(bytes);
        let c1_xtgettcap_responses = core.c1_xtgettcap_tracker.drain_pending();
        core.decscusr_tracker.observe(bytes);
        let in_progress_default_color_event = core.default_color_event_tracker.in_progress_event();
        let default_color_events = core.default_color_event_tracker.drain_pending();
        let synchronized_output_before = core
            .terminal
            .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false);
        let write_started = crate::render_prof::timer();
        self.write_pty_bytes_with_ordered_responses(
            &mut core,
            bytes,
            default_color_events,
            in_progress_default_color_event,
            c1_xtgettcap_responses,
            &mut terminal_responses,
        );
        let TerminalEffects {
            bells: terminal_bells,
            clipboard_writes,
            reported_cwd,
        } = take_terminal_effects(&mut core.terminal);
        crate::render_prof::duration_since("pty.ghostty_write", write_started);

        let has_kitty_graphics_sequence = contains_kitty_graphics_sequence(bytes);
        if has_kitty_graphics_sequence {
            debug!(pane = pane_id.raw(), "processed kitty graphics sequence");
        }
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            configure_key_encoder(&mut key_encoder, &core.terminal);
        }
        let synchronized_output = core
            .terminal
            .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false);
        if synchronized_output != synchronized_output_before {
            core.synchronized_output_epoch = core.synchronized_output_epoch.wrapping_add(1);
        }
        let request_render = !synchronized_output;
        let render_delay =
            (request_render && has_kitty_graphics_sequence).then_some(KITTY_GRAPHICS_REDRAW_SETTLE);
        if request_render {
            crate::render_prof::event("pty.request_render");
        }
        if render_delay.is_some() {
            crate::render_prof::event("pty.request_render_delayed");
        }
        if synchronized_output {
            crate::render_prof::event("pty.synchronized_output_suppressed");
        }
        ProcessBytesResult {
            request_render,
            synchronized_output,
            render_delay,
            terminal_title_changed,
            terminal_bells,
            clipboard_writes,
            reported_cwd,
            terminal_responses,
        }
    }

    /// Runtime timeout, never a drawing side effect. Release just this local
    /// parser's paint hold; no PTY input, reply, keyboard or mouse mode changes.
    pub(super) fn end_synchronized_output(&self) {
        let Ok(mut core) = self.core.lock() else {
            error!("ghostty core lock poisoned while expiring synchronized output");
            return;
        };
        if core
            .terminal
            .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false)
        {
            match core
                .terminal
                .mode_set(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT, false)
            {
                Ok(()) => {
                    core.synchronized_output_epoch = core.synchronized_output_epoch.wrapping_add(1)
                }
                Err(error) => tracing::warn!(%error, "could not expire synchronized output"),
            }
        }
    }
}

fn discard_terminal_effects(terminal: &mut crate::ghostty::Terminal) {
    // Restored history may have exercised callbacks before this live PTY write.
    // Those effects must not be delivered as live pane output.
    let _ = terminal.take_pwd_changes();
    let _ = terminal.take_bell_count();
    let _ = terminal.take_clipboard_writes();
}

struct TerminalEffects {
    bells: u16,
    clipboard_writes: Vec<Vec<u8>>,
    reported_cwd: Option<std::path::PathBuf>,
}

fn take_terminal_effects(terminal: &mut crate::ghostty::Terminal) -> TerminalEffects {
    TerminalEffects {
        bells: terminal.take_bell_count(),
        clipboard_writes: terminal.take_clipboard_writes(),
        reported_cwd: terminal
            .take_pwd_changes()
            .into_iter()
            .filter_map(|value| parse_reported_cwd(&value))
            .next_back(),
    }
}

fn contains_kitty_graphics_sequence(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|window| window == b"\x1b_G")
}
