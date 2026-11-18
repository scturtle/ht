//! Resize recovery, scroll viewport and cursor presentation state.
//! Preserve resize replay ordering and keep all terminal accesses under the same lock.

use bytes::Bytes;

use super::read::{
    ghostty_detection_text, ghostty_recent_ansi, ghostty_visible_text, DEFAULT_DETECTION_ROWS,
};
use super::{GhosttyPaneCore, GhosttyPaneTerminal, ScrollMetrics, TerminalCursorState};
use crate::pane::cursor::DecscusrTracker;

impl GhosttyPaneTerminal {
    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        if let Ok(mut core) = self.core.lock() {
            let synchronized_output_before = core
                .terminal
                .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                .unwrap_or(false);
            let offset_from_bottom = core
                .terminal
                .scrollbar()
                .ok()
                .map(|scrollbar| {
                    scrollbar
                        .total
                        .saturating_sub(scrollbar.offset + scrollbar.len)
                })
                .unwrap_or(0);
            let bottom_before_resize = ghostty_detection_text(&core.terminal)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false);
            let resize_recovery_probe_lines = usize::from(rows)
                .saturating_mul(8)
                .max(DEFAULT_DETECTION_ROWS);
            let replay_ansi = if core.terminal.active_screen().ok()
                == Some(crate::ghostty::ActiveScreen::Primary)
                && bottom_before_resize
            {
                ghostty_recent_ansi(&core.terminal, resize_recovery_probe_lines, true)
                    .ok()
                    .filter(|ansi| !ansi.trim().is_empty())
            } else {
                None
            };

            let _ = core
                .terminal
                .resize(cols, rows, cell_width_px, cell_height_px);
            let synchronized_output_after = core
                .terminal
                .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                .unwrap_or(false);
            if synchronized_output_after != synchronized_output_before {
                core.synchronized_output_epoch = core.synchronized_output_epoch.wrapping_add(1);
            }
            let terminal_responses = self.drain_pending_pty_responses();

            let bottom_is_blank = ghostty_detection_text(&core.terminal)
                .map(|text| text.trim().is_empty())
                .unwrap_or(false);
            if bottom_is_blank {
                if let Some(ansi) = replay_ansi.as_deref() {
                    core.terminal.scroll_viewport_bottom();
                    core.terminal.write(ansi.as_bytes());
                }
            }
            ghostty_set_scroll_offset_from_bottom(&mut core.terminal, offset_from_bottom);
            if offset_from_bottom > 0 {
                let mut remaining = offset_from_bottom.min(resize_recovery_probe_lines);
                while remaining > 0
                    && ghostty_visible_text(&mut core)
                        .map(|text| text.trim().is_empty())
                        .unwrap_or(false)
                {
                    core.terminal.scroll_viewport_delta(1);
                    remaining -= 1;
                }
            }
            terminal_responses
        } else {
            Vec::new()
        }
    }

    pub fn scroll_up(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_delta(-(lines as isize));
        }
    }

    pub fn scroll_down(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_delta(lines as isize);
        }
    }

    pub fn scroll_reset(&self) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_bottom();
        }
    }

    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            ghostty_set_scroll_offset_from_bottom(&mut core.terminal, lines);
        }
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        #[cfg(test)]
        self.scroll_metrics_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let scrollbar = core.terminal.scrollbar().ok()?;
        Some(ScrollMetrics {
            offset_from_bottom: scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len),
            max_offset_from_bottom: scrollbar.total.saturating_sub(scrollbar.len),
            viewport_rows: scrollbar.len,
        })
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.core.lock().is_ok_and(|core| {
            core.terminal.active_screen().ok() == Some(crate::ghostty::ActiveScreen::Alternate)
        })
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let mut core = self.core.lock().ok()?;
        let GhosttyPaneCore {
            terminal,
            render_state,
            decscusr_tracker,
            ..
        } = &mut *core;
        render_state.update(terminal).ok()?;
        cursor_state_from_render_state(render_state, decscusr_tracker)
    }
}

pub(super) fn decscusr_cursor_shape(
    style: crate::ghostty::CursorVisualStyle,
    blinking: bool,
) -> u8 {
    match (style, blinking) {
        (crate::ghostty::CursorVisualStyle::Block, true)
        | (crate::ghostty::CursorVisualStyle::BlockHollow, true) => 1,
        (crate::ghostty::CursorVisualStyle::Block, false)
        | (crate::ghostty::CursorVisualStyle::BlockHollow, false) => 2,
        (crate::ghostty::CursorVisualStyle::Underline, true) => 3,
        (crate::ghostty::CursorVisualStyle::Underline, false) => 4,
        (crate::ghostty::CursorVisualStyle::Bar, true) => 5,
        (crate::ghostty::CursorVisualStyle::Bar, false) => 6,
    }
}

pub(super) fn cursor_state_from_render_state(
    render_state: &mut crate::ghostty::RenderState,
    decscusr_tracker: &DecscusrTracker,
) -> Option<TerminalCursorState> {
    let cursor = render_state.cursor().ok()?;
    let viewport = cursor.viewport?;
    let shape = if decscusr_tracker.cursor_shape_overridden() {
        decscusr_cursor_shape(cursor.visual_style, cursor.blinking)
    } else {
        0
    };
    Some(TerminalCursorState {
        x: viewport.x,
        y: viewport.y,
        visible: cursor.visible,
        shape,
    })
}

fn ghostty_set_scroll_offset_from_bottom(
    terminal: &mut crate::ghostty::Terminal,
    offset_from_bottom: usize,
) {
    let Ok(scrollbar) = terminal.scrollbar() else {
        terminal.scroll_viewport_bottom();
        return;
    };
    let max_offset = scrollbar.total.saturating_sub(scrollbar.len);
    let offset_from_bottom = offset_from_bottom.min(max_offset);
    if offset_from_bottom == 0 {
        terminal.scroll_viewport_bottom();
    } else {
        terminal.scroll_viewport_row(max_offset - offset_from_bottom);
    }
}
