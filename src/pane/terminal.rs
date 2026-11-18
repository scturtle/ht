//! Pane terminal facade, shared state and initialization.
//! Stream, input, viewport, reads and rendering live in focused child modules.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ratatui::{layout::Rect, Frame};

use crate::layout::PaneId;

mod input;
mod read;
mod render;
mod responses;
mod stream;
mod text;
mod theme;
mod viewport;

use theme::ThemeState;

pub(crate) use text::{
    TerminalSearchDirection, TerminalSearchWindow, TerminalTextPoint, TerminalWordMotion,
};

#[cfg(test)]
mod migration_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::test_encode_key_for_app;

use super::cursor::DecscusrTracker;
use super::{
    input::configure_key_encoder,
    kitty_keyboard::KittyKeyboardTracker,
    osc::{AgentOscStateTracker, DefaultColorEventTracker, OscDebugTracker},
    xtgettcap::C1XtgettcapQueryTracker,
};

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
    pub default_color_event_tracker: DefaultColorEventTracker,
    c1_xtgettcap_tracker: C1XtgettcapQueryTracker,
    pub osc_debug_tracker: OscDebugTracker,
    pub agent_osc_state: AgentOscStateTracker,
    decscusr_tracker: DecscusrTracker,
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

    pub fn apply_host_terminal_theme(&self, theme: crate::terminal::TerminalTheme) {
        self.ghostty.apply_host_terminal_theme(theme);
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal::HostAppearance>,
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
        // The parser must store kitty images; without this it consumes the
        // sequences but retains no image.
        terminal
            .enable_kitty_graphics()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
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
                default_color_event_tracker: DefaultColorEventTracker::default(),
                c1_xtgettcap_tracker: C1XtgettcapQueryTracker::default(),
                osc_debug_tracker: OscDebugTracker::default(),
                agent_osc_state: AgentOscStateTracker::default(),
                decscusr_tracker: DecscusrTracker::default(),
            }),
            key_encoder: Mutex::new(key_encoder),
            pending_pty_responses,
        })
    }
}
