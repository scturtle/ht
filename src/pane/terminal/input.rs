//! Narrow input-mode reads, routing policy and live-state key/mouse encoding.
//! Keyboard encoding holds the core before the key encoder, just like PTY writes.

use tracing::{debug, warn};

use super::GhosttyPaneTerminal;
use crate::pane::input::{
    ghostty_key_event_from_terminal_key, ghostty_mouse_encoder_for_terminal,
    ghostty_mouse_event_from_button_kind, ghostty_mouse_event_from_motion_kind,
    ghostty_mouse_event_from_wheel_kind, legacy_shell_key, legacy_super_chord,
};

pub(super) const MODE_MOUSE_ANY_MOTION: u16 = 1003;

impl GhosttyPaneTerminal {
    pub fn keyboard_protocol(&self) -> Option<crate::input::KeyboardProtocol> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        Some(crate::input::KeyboardProtocol::from_kitty_flags(
            core.terminal.kitty_keyboard_flags().ok()? as u16,
        ))
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_BRACKETED_PASTE)
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mouse_tracking_enabled().unwrap_or(false))
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_MOUSE_SGR_PIXELS)
    }

    fn mode_enabled(&self, mode: u16) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mode_get(mode).unwrap_or(false))
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        let core = self.core.lock().ok()?;
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_reporting = core.terminal.mouse_tracking_enabled().ok()?;
        let application_cursor = core
            .terminal
            .mode_get(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS)
            .ok()?;
        let bracketed_paste = core
            .terminal
            .mode_get(crate::ghostty::MODE_BRACKETED_PASTE)
            .ok()?;
        Some(!alternate_screen && !mouse_reporting && (!application_cursor || bracketed_paste))
    }

    pub fn wheel_routing(&self) -> Option<crate::pane::WheelRouting> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_alternate_scroll = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL)
            .ok()?;
        let mouse_reporting = core.terminal.mouse_tracking_enabled().ok()?;
        Some(if mouse_reporting {
            crate::pane::WheelRouting::MouseReport
        } else if alternate_screen && mouse_alternate_scroll {
            crate::pane::WheelRouting::AlternateScroll
        } else {
            crate::pane::WheelRouting::HostScroll
        })
    }

    /// libghostty encodes every key for the pane's live keyboard modes. The only
    /// Herdr policy is the legacy shell table for panes that negotiated nothing.
    ///
    /// The terminal lock is held until the key and its grouped repeats are
    /// encoded, so a mode change from the PTY reader cannot split the decision
    /// (legacy table) from the encoder's configuration. Lock order matches
    /// output processing: terminal core, then key encoder.
    pub fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        let Ok(core) = self.core.lock() else {
            return Vec::new();
        };
        let negotiated_nothing = keyboard_negotiated_nothing(&core.terminal);
        // Do not forward Super chords as bare keys without a negotiated protocol.
        if negotiated_nothing && legacy_super_chord(&key) {
            debug!(code = ?key.code, "super chord in a pane without keyboard protocol; not forwarded");
            return Vec::new();
        }
        let Ok(mut encoder) = self.key_encoder.lock() else {
            return Vec::new();
        };

        let repeat_count = key.repeat_count;
        let first = key.with_repeat_count(1);
        let mut bytes = encode_key_with(&mut encoder, negotiated_nothing, first.clone());
        if repeat_count > 1 && first.kind != crossterm::event::KeyEventKind::Release {
            let repeated = first.with_kind(crossterm::event::KeyEventKind::Repeat);
            let repeated_bytes = encode_key_with(&mut encoder, negotiated_nothing, repeated);
            for _ in 1..repeat_count {
                bytes.extend_from_slice(&repeated_bytes);
            }
        }
        drop(encoder);
        drop(core);
        bytes
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_button_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_motion_kind(kind, 0, 0, modifiers)?,
            position,
            true,
        )
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_wheel_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    fn encode_mouse_event(
        &self,
        mut event: crate::ghostty::MouseEvent,
        position: crate::input::mouse::Position,
        require_any_motion: bool,
    ) -> Option<Vec<u8>> {
        let core = self.core.lock().ok()?;
        if require_any_motion && !core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
            return None;
        }
        let (mut encoder, (x, y)) = ghostty_mouse_encoder_for_terminal(&core.terminal, position)?;
        event.set_position(x, y);
        encoder
            .encode(&event)
            .ok()
            .filter(|bytes| !bytes.is_empty())
    }
}

/// Encode one key with an encoder configured for the same mode snapshot as
/// `negotiated_nothing`.
pub(super) fn encode_key_with(
    encoder: &mut crate::ghostty::KeyEncoder,
    negotiated_nothing: bool,
    key: crate::input::TerminalKey,
) -> Vec<u8> {
    // Before the shell table: Super+Enter must not become a plain Enter.
    if negotiated_nothing && legacy_super_chord(&key) {
        debug!(code = ?key.code, "super chord in a pane without keyboard protocol; not forwarded");
        return Vec::new();
    }
    let key = if negotiated_nothing {
        legacy_shell_key(key)
    } else {
        key
    };
    let Some(event) = ghostty_key_event_from_terminal_key(&key, negotiated_nothing) else {
        debug!(code = ?key.code, modifiers = ?key.modifiers, "key has no libghostty equivalent; not forwarded");
        return Vec::new();
    };
    encoder.encode(&event).unwrap_or_else(|err| {
        warn!(?err, code = ?key.code, "libghostty key encoding failed");
        Vec::new()
    })
}

/// True when the pane's app asked for no enhanced keyboard protocol (no Kitty
/// flags, no modifyOtherKeys 2), read from libghostty's live state.
pub(super) fn keyboard_negotiated_nothing(terminal: &crate::ghostty::Terminal) -> bool {
    terminal
        .kitty_keyboard_flags()
        .is_ok_and(|flags| flags == 0)
        && terminal
            .modify_other_keys_enabled()
            .is_ok_and(|enabled| !enabled)
}
