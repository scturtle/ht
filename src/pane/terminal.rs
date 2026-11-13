use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use ratatui::{layout::Rect, Frame};
use tracing::{debug, error, warn};

use crate::layout::PaneId;

mod diagnostics;
mod render;
mod responses;
mod text;
mod theme;

use render::ghostty_visible_hyperlinks;
use theme::ThemeState;

use text::{terminal_cell_text, RetainedTextBuffer};
pub(crate) use text::{
    TerminalSearchDirection, TerminalSearchWindow, TerminalTextPoint, TerminalWordMotion,
};

#[cfg(test)]
mod migration_tests;

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::test_encode_key_for_app;

use super::cursor::{CursorPositionSettleState, DecscusrTracker};
use super::{
    input::{
        configure_key_encoder, ghostty_key_event_from_terminal_key,
        ghostty_mouse_encoder_for_terminal, ghostty_mouse_event_from_button_kind,
        ghostty_mouse_event_from_motion_kind, ghostty_mouse_event_from_wheel_kind,
        legacy_shell_key, legacy_super_chord,
    },
    kitty_keyboard::KittyKeyboardTracker,
    osc::{
        parse_reported_cwd, AgentOscStateTracker, DefaultColorEventTracker, DefaultColorOscTracker,
        OscDebugTracker,
    },
    xtgettcap::C1XtgettcapQueryTracker,
};

const DEFAULT_DETECTION_ROWS: usize = 24;
const KITTY_GRAPHICS_REDRAW_SETTLE: Duration = Duration::from_millis(20);
const CURSOR_POSITION_SETTLE_ENABLED: bool = false;
const MODE_MOUSE_ANY_MOTION: u16 = 1003;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollMetrics {
    pub offset_from_bottom: usize,
    pub max_offset_from_bottom: usize,
    pub viewport_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    /// DECSCUSR parameter (0–6). 0 means terminal default.
    pub shape: u8,
}

fn decscusr_cursor_shape(style: crate::ghostty::CursorVisualStyle, blinking: bool) -> u8 {
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProcessBytesResult {
    pub request_render: bool,
    /// Parser mode after this DATA chunk; presentation must also honor an open update.
    pub synchronized_output: bool,
    pub render_delay: Option<Duration>,
    pub terminal_title_changed: bool,
    pub terminal_bells: u16,
    pub clipboard_writes: Vec<Vec<u8>>,
    pub reported_cwd: Option<std::path::PathBuf>,
    pub terminal_responses: Vec<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TerminalReadSnapshot {
    pub text: String,
    pub truncated: bool,
}

pub(crate) struct GhosttyPaneTerminal {
    pub(super) core: Mutex<GhosttyPaneCore>,
    #[cfg(test)]
    pub(super) scroll_metrics_reads: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(super) detection_text_reads: std::sync::atomic::AtomicUsize,
    key_encoder: Mutex<crate::ghostty::KeyEncoder>,
    pending_pty_responses: Arc<Mutex<Vec<Bytes>>>,
}

pub(crate) struct GhosttyPaneCore {
    #[cfg(test)]
    pub dirty_collection_hook: Option<Box<dyn FnOnce() + Send>>,
    pub terminal: crate::ghostty::Terminal,
    synchronized_output_epoch: u64,
    pub render_state: crate::ghostty::RenderState,
    pub kitty_keyboard: KittyKeyboardTracker,
    theme: ThemeState,
    pub default_color_tracker: DefaultColorOscTracker,
    pub default_color_event_tracker: DefaultColorEventTracker,
    c1_xtgettcap_tracker: C1XtgettcapQueryTracker,
    pub osc_debug_tracker: OscDebugTracker,
    pub agent_osc_state: AgentOscStateTracker,
    decscusr_tracker: DecscusrTracker,
    cursor_settle_state: CursorPositionSettleState,
}

pub(crate) struct PaneTerminal {
    pub(crate) ghostty: GhosttyPaneTerminal,
}

impl PaneTerminal {
    pub(crate) fn new(ghostty: GhosttyPaneTerminal) -> Self {
        Self { ghostty }
    }

    pub fn process_pty_bytes(&self, pane_id: PaneId, bytes: &[u8]) -> ProcessBytesResult {
        self.ghostty.process_pty_bytes(pane_id, bytes)
    }

    pub fn end_synchronized_output(&self) {
        self.ghostty.end_synchronized_output();
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        self.ghostty
            .resize(rows, cols, cell_width_px, cell_height_px)
    }

    pub fn scroll_up(&self, lines: usize) {
        self.ghostty.scroll_up(lines);
    }

    pub fn scroll_down(&self, lines: usize) {
        self.ghostty.scroll_down(lines);
    }

    pub fn scroll_reset(&self) {
        self.ghostty.scroll_reset();
    }

    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        self.ghostty.set_scroll_offset_from_bottom(lines);
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.ghostty.scroll_metrics()
    }

    pub(crate) fn search_text_window(
        &self,
        query: &str,
        case_sensitive: bool,
        direction: TerminalSearchDirection,
        cursor: TerminalTextPoint,
        previous: Option<(TerminalTextPoint, TerminalTextPoint)>,
        limit: usize,
    ) -> TerminalSearchWindow {
        let Some((buffer, active_screen)) = self.retained_text_buffer() else {
            return TerminalSearchWindow::default();
        };
        buffer.search_window(
            query,
            case_sensitive,
            active_screen,
            direction,
            cursor,
            previous,
            limit,
        )
    }

    pub(crate) fn word_motion_target(
        &self,
        row: u32,
        col: u16,
        motion: TerminalWordMotion,
    ) -> Option<TerminalTextPoint> {
        let core = self.ghostty.core.lock().ok()?;
        let cols = core.terminal.cols().ok()?;
        let total_rows = core.terminal.total_rows().ok()?;
        let row = usize::try_from(row).ok()?;
        if row >= total_rows {
            return None;
        }

        let mut window_rows = 64usize;
        loop {
            let (start_row, end_row) = match motion {
                TerminalWordMotion::PreviousStart | TerminalWordMotion::PreviousBigStart => {
                    (row.saturating_sub(window_rows.saturating_sub(1)), row + 1)
                }
                TerminalWordMotion::NextStart
                | TerminalWordMotion::NextEnd
                | TerminalWordMotion::NextBigStart
                | TerminalWordMotion::NextBigEnd => {
                    (row, row.saturating_add(window_rows).min(total_rows))
                }
            };
            let rows = core
                .terminal
                .screen_text_rows_range(start_row, end_row)
                .ok()?;
            let starts_in_continuation = rows
                .first()
                .is_some_and(|row| row.wrap_continuation && start_row > 0);
            let ends_in_continuation = rows
                .last()
                .is_some_and(|row| row.soft_wrapped && end_row < total_rows);
            let buffer = RetainedTextBuffer::new_words(cols, rows, u32::try_from(start_row).ok()?);
            let target = buffer.word_motion(u32::try_from(row).ok()?, col, motion);
            let needs_more_history = (motion == TerminalWordMotion::PreviousStart
                || motion == TerminalWordMotion::PreviousBigStart)
                && target
                    .is_some_and(|target| starts_in_continuation && target.row == start_row as u32);
            let needs_more_future = (motion == TerminalWordMotion::NextEnd
                || motion == TerminalWordMotion::NextBigEnd)
                && ends_in_continuation
                && target.is_some_and(|target| buffer.point_is_final_atom(target));
            if target.is_some() && !needs_more_history && !needs_more_future {
                return target;
            }

            let reached_edge = match motion {
                TerminalWordMotion::PreviousStart | TerminalWordMotion::PreviousBigStart => {
                    start_row == 0
                }
                TerminalWordMotion::NextStart
                | TerminalWordMotion::NextEnd
                | TerminalWordMotion::NextBigStart
                | TerminalWordMotion::NextBigEnd => end_row == total_rows,
            };
            if reached_edge {
                return target;
            }
            window_rows = window_rows.saturating_mul(2).min(total_rows);
        }
    }

    /// Retained-buffer coordinates, independent of the user's scroll viewport.
    pub(crate) fn text_dimensions(&self) -> Option<(u16, usize)> {
        let core = self.ghostty.core.lock().ok()?;
        Some((core.terminal.cols().ok()?, core.terminal.total_rows().ok()?))
    }

    pub(crate) fn paragraph_motion_target(
        &self,
        row: u32,
        direction: i8,
    ) -> Option<TerminalTextPoint> {
        let core = self.ghostty.core.lock().ok()?;
        let total_rows = core.terminal.total_rows().ok()?;
        let current = usize::try_from(row).ok()?;
        if current >= total_rows || direction == 0 {
            return None;
        }
        // Preserve the bounded scan of the existing copy-mode implementation.
        for distance in 1..total_rows.min(1000) {
            let candidate = if direction < 0 {
                current.checked_sub(distance)?
            } else {
                let candidate = current.saturating_add(distance);
                if candidate >= total_rows {
                    return None;
                }
                candidate
            };
            let rows = core
                .terminal
                .screen_text_rows_range(candidate, candidate + 1)
                .ok()?;
            let row = rows.first()?;
            if row.cells.iter().all(|cell| {
                terminal_cell_text(&cell.graphemes)
                    .chars()
                    .all(char::is_whitespace)
            }) {
                return Some(TerminalTextPoint {
                    row: u32::try_from(candidate).ok()?,
                    col: 0,
                });
            }
        }
        None
    }

    fn retained_text_buffer(&self) -> Option<(RetainedTextBuffer, crate::ghostty::ActiveScreen)> {
        let (cols, rows, active_screen) = {
            let core = self.ghostty.core.lock().ok()?;
            let cols = core.terminal.cols().ok()?;
            let rows = core.terminal.screen_text_rows().ok()?;
            let active_screen = core.terminal.active_screen().ok()?;
            (cols, rows, active_screen)
        };
        Some((RetainedTextBuffer::new_search(cols, rows, 0), active_screen))
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.ghostty.bracketed_paste_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.ghostty.mouse_reporting_enabled()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.ghostty.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.ghostty.plain_page_keys_use_host_scrollback()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.ghostty.alternate_screen_active()
    }

    pub fn wheel_routing(&self) -> Option<crate::pane::WheelRouting> {
        self.ghostty.wheel_routing()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.ghostty.cursor_state()
    }

    pub fn extract_selection(&self, selection: &crate::selection::Selection) -> Option<String> {
        self.ghostty.extract_selection(selection)
    }

    pub fn kitty_image_placements_with_data_filter<F>(
        &self,
        needs_data: F,
    ) -> Vec<crate::ghostty::KittyImagePlacement>
    where
        F: FnMut(crate::ghostty::KittyImageDescriptor) -> bool,
    {
        self.ghostty
            .kitty_image_placements_with_data_filter(needs_data)
    }

    pub(crate) fn kitty_image_fingerprints(&self, image_ids: &[u32]) -> Vec<Option<u64>> {
        self.ghostty.kitty_image_fingerprints(image_ids)
    }
    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.ghostty.render(frame, area, show_cursor);
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.ghostty.visible_hyperlinks(area)
    }

    pub fn apply_host_terminal_theme(&self, theme: crate::terminal_theme::TerminalTheme) {
        self.ghostty.apply_host_terminal_theme(theme);
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
    ) -> Option<Bytes> {
        self.ghostty.apply_host_terminal_appearance(appearance)
    }

    pub fn keyboard_protocol(
        &self,
        fallback: crate::input::KeyboardProtocol,
    ) -> crate::input::KeyboardProtocol {
        self.ghostty.keyboard_protocol().unwrap_or(fallback)
    }

    pub fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        self.ghostty.encode_terminal_key(key)
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_wheel(kind, position, modifiers)
    }
}

impl GhosttyPaneTerminal {
    pub fn new(mut terminal: crate::ghostty::Terminal) -> std::io::Result<Self> {
        // Same gate the retired server applied when it spawned a pane terminal:
        // without this the parser consumes kitty sequences but stores no image.
        if crate::kitty_graphics::is_enabled() {
            terminal
                .enable_kitty_graphics()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        let pending_pty_responses = Arc::new(Mutex::new(Vec::new()));
        let callback_responses = pending_pty_responses.clone();
        terminal
            .set_write_pty_callback(move |bytes| {
                if let Ok(mut responses) = callback_responses.lock() {
                    responses.push(Bytes::copy_from_slice(bytes));
                }
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let render_state =
            crate::ghostty::RenderState::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        // Callers pass a terminal with unset default colors, so an updated render
        // state would report the same baseline as an empty one. Updating here would
        // build a cell snapshot at the spawn size, which hidden panes keep alive.
        let initial_colors = render_state.colors().ok();
        let initial_default_foreground = initial_colors.map(|colors| colors.foreground);
        let initial_default_background = initial_colors.map(|colors| colors.background);
        let mut key_encoder =
            crate::ghostty::KeyEncoder::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        configure_key_encoder(&mut key_encoder, &terminal);
        Ok(Self {
            #[cfg(test)]
            scroll_metrics_reads: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            detection_text_reads: std::sync::atomic::AtomicUsize::new(0),
            core: Mutex::new(GhosttyPaneCore {
                #[cfg(test)]
                dirty_collection_hook: None,
                terminal,
                synchronized_output_epoch: 0,
                render_state,
                kitty_keyboard: KittyKeyboardTracker::default(),
                theme: ThemeState {
                    initial_foreground: initial_default_foreground,
                    initial_background: initial_default_background,
                    ..ThemeState::default()
                },
                default_color_tracker: DefaultColorOscTracker::default(),
                default_color_event_tracker: DefaultColorEventTracker::default(),
                c1_xtgettcap_tracker: C1XtgettcapQueryTracker::default(),
                osc_debug_tracker: OscDebugTracker::default(),
                agent_osc_state: AgentOscStateTracker::default(),
                decscusr_tracker: DecscusrTracker::default(),
                cursor_settle_state: CursorPositionSettleState::default(),
            }),
            key_encoder: Mutex::new(key_encoder),
            pending_pty_responses,
        })
    }

    pub fn process_pty_bytes(&self, pane_id: PaneId, bytes: &[u8]) -> ProcessBytesResult {
        crate::render_prof::counter("pty.bytes", bytes.len() as u64);
        let Ok(mut core) = self.core.lock() else {
            error!(pane = pane_id.raw(), "ghostty core lock poisoned in reader");
            return ProcessBytesResult::default();
        };

        discard_terminal_effects(&mut core.terminal);
        core.default_color_tracker.observe(bytes);

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

        let has_kitty_graphics_sequence =
            crate::kitty_graphics::is_enabled() && contains_kitty_graphics_sequence(bytes);
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
        // Intermediate synchronized-frame positions must not become settled cursors.
        if CURSOR_POSITION_SETTLE_ENABLED && !synchronized_output {
            let cursor_started = crate::render_prof::timer();
            let cursor_after_write = current_cursor_state(&mut core);
            crate::render_prof::duration_since("pty.cursor_state_update", cursor_started);
            core.cursor_settle_state
                .observe(cursor_after_write, Instant::now());
        }

        let request_render = !synchronized_output;
        let render_delay = render_delay_after_pty_write(
            synchronized_output,
            has_kitty_graphics_sequence,
            core.cursor_settle_state.render_delay(),
            CURSOR_POSITION_SETTLE_ENABLED,
        );
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

    pub fn alternate_screen_active(&self) -> bool {
        self.core.lock().is_ok_and(|core| {
            core.terminal.active_screen().ok() == Some(crate::ghostty::ActiveScreen::Alternate)
        })
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

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let mut core = self.core.lock().ok()?;
        let current = current_cursor_state(&mut core);
        effective_cursor_state(&mut core, current)
    }

    /// Runtime timeout, never a drawing side effect. Release just this local
    /// parser's paint hold; no PTY input, reply, keyboard or mouse mode changes.
    fn end_synchronized_output(&self) {
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

    pub fn extract_selection(&self, selection: &crate::selection::Selection) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_extract_selection(&core.terminal, selection).ok())
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_visible_hyperlinks(&mut core, area).ok())
            .unwrap_or_default()
    }

    pub fn kitty_image_placements_with_data_filter<F>(
        &self,
        needs_data: F,
    ) -> Vec<crate::ghostty::KittyImagePlacement>
    where
        F: FnMut(crate::ghostty::KittyImageDescriptor) -> bool,
    {
        self.core
            .lock()
            .ok()
            .and_then(|core| {
                core.terminal
                    .kitty_image_placements_with_data_filter(needs_data)
                    .ok()
            })
            .unwrap_or_default()
    }

    pub(crate) fn kitty_image_fingerprints(&self, image_ids: &[u32]) -> Vec<Option<u64>> {
        self.core
            .lock()
            .ok()
            .and_then(|core| core.terminal.kitty_image_fingerprints(image_ids).ok())
            .unwrap_or_else(|| vec![None; image_ids.len()])
    }
}

/// Encode one key with an encoder configured for the same mode snapshot as
/// `negotiated_nothing`.
fn encode_key_with(
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
fn keyboard_negotiated_nothing(terminal: &crate::ghostty::Terminal) -> bool {
    terminal
        .kitty_keyboard_flags()
        .is_ok_and(|flags| flags == 0)
        && terminal
            .modify_other_keys_enabled()
            .is_ok_and(|enabled| !enabled)
}

fn effective_cursor_state(
    core: &mut GhosttyPaneCore,
    current: Option<TerminalCursorState>,
) -> Option<TerminalCursorState> {
    if !CURSOR_POSITION_SETTLE_ENABLED {
        return current;
    }
    core.cursor_settle_state
        .reported_cursor(current, Instant::now())
}

fn render_delay_after_pty_write(
    synchronized_output: bool,
    has_kitty_graphics_sequence: bool,
    cursor_position_settle_delay: Option<Duration>,
    cursor_position_settle_enabled: bool,
) -> Option<Duration> {
    if synchronized_output {
        None
    } else {
        let cursor_delay = cursor_position_settle_enabled
            .then_some(cursor_position_settle_delay)
            .flatten();
        cursor_delay.max(has_kitty_graphics_sequence.then_some(KITTY_GRAPHICS_REDRAW_SETTLE))
    }
}

fn current_cursor_state(core: &mut GhosttyPaneCore) -> Option<TerminalCursorState> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        decscusr_tracker,
        ..
    } = core;
    render_state.update(terminal).ok()?;
    cursor_state_from_render_state(render_state, decscusr_tracker)
}

fn cursor_state_from_render_state(
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

fn ghostty_visible_text(core: &mut GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal)?;
    let mut row_iterator = crate::ghostty::RowIterator::new()?;
    let mut row_cells = crate::ghostty::RowCells::new()?;
    let mut rows = render_state.populate_row_iterator(&mut row_iterator)?;
    let mut lines = Vec::new();
    while rows.next() {
        let mut cells = rows.populate_cells(&mut row_cells)?;
        lines.push(ghostty_line_from_cells(&mut cells)?);
    }
    trim_trailing_blank_rows(&mut lines);
    Ok(lines_to_text(lines))
}

fn ghostty_detection_text(
    terminal: &crate::ghostty::Terminal,
) -> Result<String, crate::ghostty::Error> {
    ghostty_detection_text_for_cache(terminal).map(|(text, _)| text)
}

fn ghostty_detection_text_for_cache(
    terminal: &crate::ghostty::Terminal,
) -> Result<(String, bool), crate::ghostty::Error> {
    let lines = terminal
        .rows()
        .ok()
        .map(|rows| usize::from(rows).max(1))
        .unwrap_or(DEFAULT_DETECTION_ROWS);
    let text = ghostty_recent_text_for_terminal(terminal, lines)?;
    Ok(finish_detection_text(text))
}

fn finish_detection_text(text: String) -> (String, bool) {
    let reusable = !text.trim().is_empty();
    (text, reusable)
}

fn ghostty_recent_ansi(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<String, crate::ghostty::Error> {
    ghostty_recent_ansi_snapshot(terminal, lines, unwrap).map(|snapshot| snapshot.text)
}

fn ghostty_recent_ansi_snapshot(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_ansi_for_terminal(terminal, lines, unwrap)?;
    Ok(finish_recent_snapshot(terminal, text, lines))
}

fn finish_recent_snapshot(
    terminal: &crate::ghostty::Terminal,
    text: String,
    lines: usize,
) -> TerminalReadSnapshot {
    // Recent read limits are measured in rendered rows, including blank or styled rows.
    TerminalReadSnapshot {
        text,
        truncated: terminal
            .total_rows()
            .is_ok_and(|total_rows| total_rows > lines),
    }
}

fn ghostty_recent_text_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, _)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    let mut rows = Vec::with_capacity(end.saturating_sub(start).saturating_add(1));
    for y in start..=end {
        rows.push(ghostty_screen_row(terminal, y as u32)?);
    }
    trim_trailing_blank_rows(&mut rows);
    Ok(recent_text_from_rows(&rows, lines))
}

fn ghostty_recent_ansi_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, cols)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    terminal.read_ansi_screen(
        (0, start as u32),
        (cols.saturating_sub(1), end as u32),
        false,
        unwrap,
    )
}

fn ghostty_recent_read_range(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<Option<(usize, usize, u16)>, crate::ghostty::Error> {
    let total_rows = terminal.total_rows()?;
    let cols = terminal.cols()?;
    if total_rows == 0 || cols == 0 || lines == 0 {
        return Ok(None);
    }

    let physical_end = total_rows.saturating_sub(1);
    if terminal.active_screen()? != crate::ghostty::ActiveScreen::Primary {
        let start = physical_end.saturating_add(1).saturating_sub(lines);
        return Ok(Some((start, physical_end, cols)));
    }

    let rows = usize::from(terminal.rows()?);
    if rows == 0 {
        return Ok(None);
    }
    let viewport_start = total_rows.saturating_sub(rows);
    let cursor_row = viewport_start
        .saturating_add(usize::from(terminal.cursor_y()?))
        .min(total_rows.saturating_sub(1));
    let mut last_content_row = None;
    for row in (viewport_start..total_rows).rev() {
        if !ghostty_screen_row(terminal, row as u32)?.trim().is_empty() {
            last_content_row = Some(row);
            break;
        }
    }
    let end = last_content_row
        .map(|row| row.max(cursor_row))
        .unwrap_or_else(|| total_rows.saturating_sub(1));
    let start = end.saturating_add(1).saturating_sub(lines);
    Ok(Some((start, end, cols)))
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

fn ghostty_extract_selection(
    terminal: &crate::ghostty::Terminal,
    selection: &crate::selection::Selection,
) -> Result<String, crate::ghostty::Error> {
    let ((start_row, start_col), (end_row, end_col)) = selection.ordered_cells();
    terminal.read_text_screen((start_col, start_row), (end_col, end_row), false)
}

fn ghostty_screen_row(
    terminal: &crate::ghostty::Terminal,
    y: u32,
) -> Result<String, crate::ghostty::Error> {
    let mut line = String::new();
    // Keep one page lookup per row and reuse grapheme storage across its cells.
    terminal.for_each_screen_row_cell(y, |wide, graphemes| {
        if wide == crate::ghostty::CellWide::SpacerTail {
            return;
        }
        if graphemes.is_empty()
            || graphemes.first().copied() == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER)
        {
            line.push(' ');
        } else {
            for &codepoint in graphemes {
                if let Some(ch) = char::from_u32(codepoint) {
                    line.push(ch);
                }
            }
        }
    })?;
    Ok(line.trim_end().to_string())
}

fn ghostty_line_from_cells(
    cells: &mut crate::ghostty::RowCellIter<'_>,
) -> Result<String, crate::ghostty::Error> {
    let mut line = String::new();
    while cells.next() {
        line.push_str(&ghostty_cell_symbol(cells)?);
    }
    Ok(line.trim_end().to_string())
}

fn ghostty_cell_symbol(
    cells: &crate::ghostty::RowCellIter<'_>,
) -> Result<String, crate::ghostty::Error> {
    if cells.wide()? == crate::ghostty::CellWide::SpacerTail {
        return Ok(String::new());
    }
    let text = cells.grapheme_text()?;
    if text.chars().next().map(u32::from) == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER) {
        return Ok(" ".to_string());
    }
    if text.is_empty() {
        return Ok(" ".to_string());
    }
    Ok(text)
}

fn lines_to_text(lines: Vec<String>) -> String {
    let text = lines.join("\n");
    if text.is_empty() {
        text
    } else {
        format!("{text}\n")
    }
}

pub(super) fn trim_trailing_blank_rows(rows: &mut Vec<String>) {
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
}

fn recent_text_from_rows(rows: &[String], lines: usize) -> String {
    let start = rows.len().saturating_sub(lines);
    let text = rows[start..].join("\n");
    if text.is_empty() {
        text
    } else {
        format!("{text}\n")
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
