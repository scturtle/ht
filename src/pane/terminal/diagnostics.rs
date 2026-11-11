//! Read-only, on-demand inspection; never consumes renderer dirty state.
use super::*;
use serde_json::{json, Value};

impl PaneTerminal {
    pub(crate) fn diagnostic_snapshot(&self) -> Value {
        let Ok(core) = self.ghostty.core.try_lock() else {
            return json!({"error":"terminal core is locked or poisoned"});
        };
        let cols = core.terminal.cols().ok();
        let rows = core.terminal.rows().ok();
        let mut modes = serde_json::Map::new();
        for (name, number) in [
            (
                "synchronized_output",
                crate::ghostty::MODE_SYNCHRONIZED_OUTPUT,
            ),
            (
                "application_cursor",
                crate::ghostty::MODE_APPLICATION_CURSOR_KEYS,
            ),
            ("bracketed_paste", crate::ghostty::MODE_BRACKETED_PASTE),
            ("focus_events", crate::ghostty::MODE_FOCUS_EVENT),
            ("mouse_x10", 9),
            ("mouse_normal", 1000),
            ("mouse_button", 1002),
            ("mouse_any", 1003),
            ("mouse_sgr", crate::ghostty::MODE_MOUSE_SGR),
            ("mouse_pixels", crate::ghostty::MODE_MOUSE_SGR_PIXELS),
            (
                "alternate_scroll",
                crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL,
            ),
        ] {
            modes.insert(name.into(), json!(core.terminal.mode_get(number).ok()));
        }
        let text = cols.zip(rows).map(|(cols, rows)| {
            core.terminal
                .read_text_viewport(
                    (0, 0),
                    (cols.saturating_sub(1), u32::from(rows.saturating_sub(1))),
                    true,
                )
                .map_err(|e| e.to_string())
        });
        json!({
            "cols":cols,"rows":rows,"modes":modes,
            "synchronized_output_epoch":core.synchronized_output_epoch,
            "keyboard_flags":core.terminal.kitty_keyboard_flags().ok(),
            "scroll":core.terminal.scrollbar().ok().map(|s| json!({"total":s.total,"offset":s.offset,"rows":s.len})),
            "active_screen":core.terminal.active_screen().ok().map(|s| format!("{s:?}")),
            "viewport":text.map(|r| match r { Ok(text)=>json!({"text":text}), Err(error)=>json!({"error":error}) }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_snapshot_does_not_recover_or_clean_a_synchronized_frame() {
        let terminal = crate::ghostty::Terminal::new(40, 10, 100).unwrap();
        let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
        let id = PaneId::alloc();
        pane.process_pty_bytes(id, b"\x1b[?2026hFROZEN");
        let before = pane
            .ghostty
            .core
            .lock()
            .unwrap()
            .render_state
            .dirty()
            .unwrap();
        let snapshot = pane.diagnostic_snapshot();
        assert_eq!(snapshot["modes"]["synchronized_output"], true);
        assert!(snapshot["viewport"]["text"]
            .as_str()
            .unwrap()
            .contains("FROZEN"));
        let core = pane.ghostty.core.lock().unwrap();
        assert!(core
            .terminal
            .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap());
        assert_eq!(core.render_state.dirty().unwrap(), before);
    }

    #[test]
    fn locked_terminal_snapshot_is_nonblocking() {
        let terminal = crate::ghostty::Terminal::new(40, 10, 100).unwrap();
        let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
        let _held = pane.ghostty.core.lock().unwrap();
        assert!(pane.diagnostic_snapshot()["error"].is_string());
    }
}
