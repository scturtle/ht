//! Terminal unit tests and test-only inspection/replay helpers.

use super::render::{
    ghostty_blank_symbol_for_width, ghostty_buffer_symbol_into, ghostty_cell_color,
    ghostty_cell_style, ghostty_color, ghostty_default_bg, ghostty_default_fg,
    ghostty_default_style, is_halfwidth_katakana_voiced_grapheme, PaletteOverrides,
};
use super::text::TerminalTextMatch;
use super::*;
use crate::pane::cursor::CURSOR_POSITION_SETTLE;
use crate::protocol::CellData;
use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

const MODE_MOUSE_X10: u16 = 9;
const MODE_MOUSE_PRESS_RELEASE: u16 = 1000;
const MODE_MOUSE_BUTTON_MOTION: u16 = 1002;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalDirtyPatch {
    pub rows: Vec<(u16, Vec<CellData>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalDirtyPatchOutcome {
    Clean,
    Patch(TerminalDirtyPatch),
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputState {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub mouse_protocol_mode: crate::input::MouseProtocolMode,
    pub mouse_protocol_encoding: crate::input::MouseProtocolEncoding,
    pub mouse_alternate_scroll: bool,
    #[serde(default)]
    pub modify_other_keys: bool,
    #[serde(default)]
    pub color_scheme_reporting: bool,
}

impl InputState {
    pub fn mouse_reporting_enabled(self) -> bool {
        self.mouse_protocol_mode.reporting_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(self) -> bool {
        !self.alternate_screen
            && !self.mouse_reporting_enabled()
            // Bracketed paste distinguishes zsh's line editor (where it's on)
            // from e.g. less -X (where it's off).
            && (!self.application_cursor || self.bracketed_paste)
    }
}

fn ghostty_collect_dirty_patch(
    core: &mut GhosttyPaneCore,
    area_width: u16,
    area_height: u16,
) -> TerminalDirtyPatchOutcome {
    let prof_started = crate::render_prof::timer();
    macro_rules! finish {
        ($outcome:expr) => {{
            let outcome = $outcome;
            if let Some(started) = prof_started {
                crate::render_prof::duration("dirty_collect.total", started.elapsed());
                match &outcome {
                    TerminalDirtyPatchOutcome::Clean => {
                        crate::render_prof::event("dirty_collect.clean");
                    }
                    TerminalDirtyPatchOutcome::Fallback => {
                        crate::render_prof::event("dirty_collect.fallback");
                    }
                    TerminalDirtyPatchOutcome::Patch(patch) => {
                        crate::render_prof::event("dirty_collect.patch");
                        crate::render_prof::counter("dirty_collect.rows", patch.rows.len() as u64);
                        let cells = patch.rows.iter().map(|(_, cells)| cells.len() as u64).sum();
                        crate::render_prof::counter("dirty_collect.cells", cells);
                    }
                }
            }
            return outcome;
        }};
    }
    macro_rules! fallback {
        ($reason:literal) => {{
            crate::render_prof::event(concat!("dirty_fallback.", $reason));
            finish!(TerminalDirtyPatchOutcome::Fallback);
        }};
    }

    let host_theme = core.theme.host;
    let initial_default_foreground = core.theme.initial_foreground;
    let initial_default_background = core.theme.initial_background;
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    if render_state.update(terminal).is_err() {
        fallback!("render_state_update_error");
    }
    match render_state.dirty() {
        Ok(crate::ghostty::Dirty::Clean) => finish!(TerminalDirtyPatchOutcome::Clean),
        Ok(crate::ghostty::Dirty::Partial | crate::ghostty::Dirty::Full) => {}
        Err(_) => fallback!("dirty_read_error"),
    }

    let colors = render_state.colors().ok();
    let default_bg = colors
        .and_then(|c| ghostty_default_bg(c.background, host_theme, initial_default_background));
    let default_fg = colors
        .and_then(|c| ghostty_default_fg(c.foreground, host_theme, initial_default_foreground));
    let resolved_fg = colors.map(|c| ghostty_color(c.foreground));
    let resolved_bg = colors.map(|c| ghostty_color(c.background));
    let palette_overrides = colors
        .zip(terminal.default_palette().ok())
        .and_then(|(colors, default)| PaletteOverrides::new(&colors.palette, &default));
    let hide_kitty_placeholders = crate::kitty_graphics::is_enabled();

    let Ok(mut row_iterator) = crate::ghostty::RowIterator::new() else {
        fallback!("row_iterator_new_error");
    };
    let Ok(mut row_cells) = crate::ghostty::RowCells::new() else {
        fallback!("row_cells_new_error");
    };
    let Ok(mut rows) = render_state.populate_row_iterator(&mut row_iterator) else {
        fallback!("populate_rows_error");
    };
    let mut grapheme_bytes = Vec::new();
    let mut symbol_scratch = String::new();
    let mut patch_rows = Vec::new();
    let blank = blank_cell_data(default_fg, default_bg);
    while let Some(y) = rows.next_dirty() {
        if y >= area_height {
            break;
        }
        match rows.selection() {
            Ok(None) => {}
            Ok(Some(_)) => fallback!("row_selection_present"),
            Err(_) => fallback!("row_selection_error"),
        }
        let Ok(mut cells) = rows.populate_cells(&mut row_cells) else {
            fallback!("populate_cells_error");
        };
        let mut patch_cells = Vec::with_capacity(usize::from(area_width));
        let mut x = 0u16;
        while x < area_width && cells.next() {
            match cells.is_default_blank() {
                Ok(true) => {
                    patch_cells.push(blank.clone());
                    x += 1;
                    continue;
                }
                Ok(false) => {}
                Err(_) => fallback!("raw_cell_error"),
            }
            let Ok(basic) = cells.basic_data() else {
                fallback!("basic_data_error");
            };
            if basic.has_hyperlink {
                fallback!("hyperlink_present");
            }
            let style = ghostty_cell_style(
                &cells,
                &basic,
                default_fg,
                default_bg,
                resolved_fg,
                resolved_bg,
                palette_overrides.as_ref(),
            );
            let symbol = match ghostty_buffer_symbol_into(
                &cells,
                basic.wide,
                hide_kitty_placeholders,
                &mut grapheme_bytes,
                &mut symbol_scratch,
            ) {
                Ok(symbol) => symbol.to_owned(),
                Err(_) => ghostty_blank_symbol_for_width(basic.wide).to_owned(),
            };
            patch_cells.push(cell_data_from_style(symbol, style));
            x += 1;
        }
        patch_cells.resize(usize::from(area_width), blank.clone());
        patch_rows.push((y, patch_cells));
    }

    // Nothing above mutates dirty state. Only clear it after every row has
    // been collected successfully, so a safety fallback leaves the next
    // collection with the same information.
    if !patch_rows.is_empty() {
        let Ok(mut clear_row_iterator) = crate::ghostty::RowIterator::new() else {
            fallback!("clear_row_iterator_new_error");
        };
        let Ok(mut clear_rows) = render_state.populate_row_iterator(&mut clear_row_iterator) else {
            fallback!("clear_populate_rows_error");
        };
        while let Some(y) = clear_rows.next_dirty() {
            if y >= area_height {
                break;
            }
            if clear_rows.clear_dirty().is_err() {
                fallback!("clear_dirty_error");
            }
        }
    }
    if render_state
        .set_dirty(crate::ghostty::Dirty::Clean)
        .is_err()
    {
        fallback!("set_clean_error");
    }

    finish!(TerminalDirtyPatchOutcome::Patch(TerminalDirtyPatch {
        rows: patch_rows
    }));
}

fn ghostty_visible_ansi(core: &GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let rows = core.terminal.rows()?;
    let cols = core.terminal.cols()?;
    if rows == 0 || cols == 0 {
        return Ok(String::new());
    }
    core.terminal.read_ansi_viewport(
        (0, 0),
        (cols.saturating_sub(1), u32::from(rows.saturating_sub(1))),
        false,
    )
}

fn ghostty_recent_text_snapshot(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_text_for_terminal(terminal, lines)?;
    Ok(finish_recent_snapshot(terminal, text, lines))
}

fn ghostty_recent_text_unwrapped_snapshot(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_text_unwrapped_for_terminal(terminal, lines)?;
    Ok(finish_recent_snapshot(terminal, text, lines))
}

fn ghostty_recent_text_unwrapped_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, cols)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    terminal.read_text_screen(
        (0, start as u32),
        (cols.saturating_sub(1), end as u32),
        false,
    )
}

pub(super) fn ghostty_normalize_buffer_symbol(
    symbol: &str,
    wide: crate::ghostty::CellWide,
) -> String {
    let expected_width = match wide {
        crate::ghostty::CellWide::Wide => 2,
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => 1,
        crate::ghostty::CellWide::SpacerTail => 0,
    };
    let actual_width = symbol.width();
    if actual_width == expected_width {
        return symbol.to_string();
    }

    if wide == crate::ghostty::CellWide::Narrow && actual_width == 2 {
        return symbol.to_string();
    }
    if wide == crate::ghostty::CellWide::Wide && is_halfwidth_katakana_voiced_grapheme(symbol) {
        return symbol.to_string();
    }

    ghostty_blank_symbol_for_width(wide).to_string()
}

fn blank_cell_data(default_fg: Option<Color>, default_bg: Option<Color>) -> CellData {
    cell_data_from_style(
        " ".to_string(),
        ghostty_default_style(default_fg, default_bg),
    )
}

fn cell_data_from_style(symbol: String, style: Style) -> CellData {
    CellData {
        symbol,
        fg: crate::protocol::color_to_u32(style.fg.unwrap_or(Color::Reset)),
        bg: crate::protocol::color_to_u32(style.bg.unwrap_or(Color::Reset)),
        modifier: crate::protocol::modifier_to_u16(style.add_modifier),
        skip: false,
        hyperlink: None,
    }
}

fn should_probe_host_terminal_theme_restore(core: &GhosttyPaneCore) -> bool {
    if core.theme.transient_owner_pgid.is_none() || core.theme.host.is_empty() {
        return false;
    }

    !core
        .terminal
        .active_screen()
        .map(|screen| screen == crate::ghostty::ActiveScreen::Alternate)
        .unwrap_or(false)
}

/// Encode one `key` for the live keyboard modes after the app wrote
/// `app_output` (for example a Kitty keyboard push).
pub(crate) fn test_encode_key_for_app(
    app_output: &[u8],
    key: crate::input::TerminalKey,
) -> Vec<u8> {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).expect("terminal");
    let pane = GhosttyPaneTerminal::new(terminal).expect("pane terminal");
    pane.process_pty_bytes(PaneId::from_raw(1), app_output);
    pane.encode_terminal_key_once(key)
}

impl PaneTerminal {
    pub fn input_state(&self) -> Option<InputState> {
        self.ghostty.input_state()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        self.ghostty.screen_text_snapshot()
    }

    pub fn visible_text(&self) -> String {
        self.ghostty.visible_text()
    }

    pub fn detection_text(&self) -> String {
        self.ghostty.detection_text()
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_text_snapshot(lines)
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        self.ghostty.recent_unwrapped_ansi(lines)
    }

    pub fn collect_dirty_patch(
        &self,
        area_width: u16,
        area_height: u16,
    ) -> TerminalDirtyPatchOutcome {
        self.ghostty.collect_dirty_patch(area_width, area_height)
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.ghostty.terminal_title()
    }
}

impl GhosttyPaneTerminal {
    pub fn has_transient_default_color_override(&self) -> bool {
        self.core
            .lock()
            .map(|core| core.theme.transient_owner_pgid.is_some())
            .unwrap_or(false)
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|core| core.agent_osc_state.terminal_title().map(str::to_string))
    }

    pub fn seed_history_ansi(&self, ansi: &str) {
        if ansi.is_empty() {
            return;
        }
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        core.kitty_keyboard.observe(ansi.as_bytes());
        core.terminal.write(ansi.as_bytes());
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            configure_key_encoder(&mut key_encoder, &core.terminal);
        }
    }

    pub fn seed_handoff_input_state(&self, input_state: InputState) {
        let Ok(mut core) = self.core.lock() else {
            return;
        };

        if input_state.alternate_screen {
            core.terminal.write(b"\x1b[?1049h");
        }
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_APPLICATION_CURSOR_KEYS,
            input_state.application_cursor,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_BRACKETED_PASTE,
            input_state.bracketed_paste,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_FOCUS_EVENT,
            input_state.focus_reporting,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL,
            input_state.mouse_alternate_scroll,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_COLOR_SCHEME_REPORT,
            input_state.color_scheme_reporting,
        );

        core.terminal
            .write(b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        let mouse_mode_ansi: Option<&[u8]> = match input_state.mouse_protocol_mode {
            crate::input::MouseProtocolMode::None => None,
            crate::input::MouseProtocolMode::Press => Some(b"\x1b[?9h"),
            crate::input::MouseProtocolMode::PressRelease => Some(b"\x1b[?1000h"),
            crate::input::MouseProtocolMode::ButtonMotion => Some(b"\x1b[?1002h"),
            crate::input::MouseProtocolMode::AnyMotion => Some(b"\x1b[?1003h"),
        };
        if let Some(ansi) = mouse_mode_ansi {
            core.terminal.write(ansi);
        }

        core.terminal.write(b"\x1b[?1005l\x1b[?1006l\x1b[?1016l");
        let mouse_encoding_ansi: Option<&[u8]> = match input_state.mouse_protocol_encoding {
            crate::input::MouseProtocolEncoding::Default => None,
            crate::input::MouseProtocolEncoding::Utf8 => Some(b"\x1b[?1005h"),
            crate::input::MouseProtocolEncoding::Sgr => Some(b"\x1b[?1006h"),
            crate::input::MouseProtocolEncoding::SgrPixels => Some(b"\x1b[?1016h"),
        };
        if let Some(ansi) = mouse_encoding_ansi {
            core.terminal.write(ansi);
        }

        if input_state.modify_other_keys {
            core.kitty_keyboard.observe(b"\x1b[>4;2m");
            core.terminal.write(b"\x1b[>4;2m");
        }

        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            configure_key_encoder(&mut key_encoder, &core.terminal);
        }
    }

    pub fn seed_keyboard_protocol_flags(&self, flags: u16) {
        if flags == 0 {
            return;
        }
        self.seed_keyboard_protocol_ansi(&format!("\x1b[>{flags}u"));
    }

    pub fn seed_keyboard_protocol_ansi(&self, ansi: &str) {
        if ansi.is_empty() {
            return;
        }
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        core.kitty_keyboard.observe(ansi.as_bytes());
        core.terminal.write(ansi.as_bytes());
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            configure_key_encoder(&mut key_encoder, &core.terminal);
        }
    }

    pub fn kitty_keyboard_state_ansi(&self) -> Option<String> {
        let core = self.core.lock().ok()?;
        core.kitty_keyboard.replay_ansi()
    }

    /// libghostty tracks only level 2; like Ghostty, level 1 behaves as if the
    /// app negotiated nothing.
    pub fn modify_other_keys_level(&self) -> u8 {
        self.core.lock().map_or(0, |core| {
            if core.terminal.modify_other_keys_enabled().unwrap_or(false) {
                2
            } else {
                0
            }
        })
    }

    // This aggregate snapshot performs multiple terminal queries. Pane-scaled
    // callers should add a narrow accessor instead.
    pub fn input_state(&self) -> Option<InputState> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let application_cursor = core
            .terminal
            .mode_get(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS)
            .ok()?;
        let bracketed_paste = core
            .terminal
            .mode_get(crate::ghostty::MODE_BRACKETED_PASTE)
            .ok()?;
        let focus_reporting = core
            .terminal
            .mode_get(crate::ghostty::MODE_FOCUS_EVENT)
            .ok()?;
        let mouse_sgr = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_SGR)
            .ok()?;
        let mouse_utf8 = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_UTF8)
            .ok()?;
        let mouse_sgr_pixels = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_SGR_PIXELS)
            .ok()?;
        let mouse_alternate_scroll = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL)
            .ok()?;
        let mouse_protocol_mode = if core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
            crate::input::MouseProtocolMode::AnyMotion
        } else if core.terminal.mode_get(MODE_MOUSE_BUTTON_MOTION).ok()? {
            crate::input::MouseProtocolMode::ButtonMotion
        } else if core.terminal.mode_get(MODE_MOUSE_PRESS_RELEASE).ok()? {
            crate::input::MouseProtocolMode::PressRelease
        } else if core.terminal.mode_get(MODE_MOUSE_X10).ok()? {
            crate::input::MouseProtocolMode::Press
        } else {
            crate::input::MouseProtocolMode::None
        };
        let mouse_protocol_encoding = if mouse_sgr_pixels {
            crate::input::MouseProtocolEncoding::SgrPixels
        } else if mouse_sgr {
            crate::input::MouseProtocolEncoding::Sgr
        } else if mouse_utf8 {
            crate::input::MouseProtocolEncoding::Utf8
        } else {
            crate::input::MouseProtocolEncoding::Default
        };
        Some(InputState {
            alternate_screen,
            application_cursor,
            bracketed_paste,
            focus_reporting,
            mouse_protocol_mode,
            mouse_protocol_encoding,
            mouse_alternate_scroll,
            modify_other_keys: core.terminal.modify_other_keys_enabled().ok()?,
            color_scheme_reporting: core
                .terminal
                .mode_get(crate::ghostty::MODE_COLOR_SCHEME_REPORT)
                .ok()?,
        })
    }

    pub(crate) fn synchronized_output_state(&self) -> (bool, u64) {
        self.core
            .lock()
            .map(|core| {
                (
                    core.terminal
                        .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                        .unwrap_or(false),
                    core.synchronized_output_epoch,
                )
            })
            .unwrap_or((true, 0))
    }

    fn encode_terminal_key_once(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        let Ok(core) = self.core.lock() else {
            return Vec::new();
        };
        let negotiated_nothing = keyboard_negotiated_nothing(&core.terminal);
        let Ok(mut encoder) = self.key_encoder.lock() else {
            return Vec::new();
        };
        encode_key_with(&mut encoder, negotiated_nothing, key)
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        let core = self.core.lock().ok()?;
        Some((
            core.terminal.active_screen().ok()?,
            core.terminal.cols().ok()?,
            core.terminal.screen_text_rows().ok()?,
        ))
    }

    pub fn visible_text(&self) -> String {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_visible_text(&mut core).ok())
            .unwrap_or_default()
    }

    pub fn visible_ansi(&self) -> String {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_visible_ansi(&core).ok())
            .unwrap_or_default()
    }

    pub fn detection_text(&self) -> String {
        self.detection_text_for_cache().0
    }

    pub(crate) fn detection_text_for_cache(&self) -> (String, bool) {
        self.detection_text_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_detection_text_for_cache(&core.terminal).ok())
            .unwrap_or_default()
    }

    pub fn recent_text(&self, lines: usize) -> String {
        self.recent_text_snapshot(lines).text
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_recent_text_snapshot(&core.terminal, lines).ok())
            .unwrap_or_default()
    }

    pub fn recent_ansi(&self, lines: usize) -> String {
        self.recent_ansi_snapshot(lines).text
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_recent_ansi_snapshot(&core.terminal, lines, false).ok())
            .unwrap_or_default()
    }

    pub fn recent_unwrapped_text(&self, lines: usize) -> String {
        self.recent_unwrapped_text_snapshot(lines).text
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_recent_text_unwrapped_snapshot(&core.terminal, lines).ok())
            .unwrap_or_default()
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        self.recent_unwrapped_ansi_snapshot(lines).text
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_recent_ansi_snapshot(&core.terminal, lines, true).ok())
            .unwrap_or_default()
    }

    pub fn collect_dirty_patch(
        &self,
        area_width: u16,
        area_height: u16,
    ) -> TerminalDirtyPatchOutcome {
        self.core
            .lock()
            .ok()
            .map(|mut core| {
                if core
                    .terminal
                    .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                    .unwrap_or(false)
                {
                    return TerminalDirtyPatchOutcome::Fallback;
                }
                if let Some(hook) = core.dirty_collection_hook.take() {
                    hook();
                }
                ghostty_collect_dirty_patch(&mut core, area_width, area_height)
            })
            .unwrap_or(TerminalDirtyPatchOutcome::Fallback)
    }
}

#[test]
fn plain_page_keys_host_scroll_for_shell_like_decckm_with_bracketed_paste() {
    assert!(InputState {
        alternate_screen: false,
        application_cursor: true,
        bracketed_paste: true,
        focus_reporting: false,
        mouse_protocol_mode: crate::input::MouseProtocolMode::None,
        mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Default,
        mouse_alternate_scroll: false,
        modify_other_keys: false,
        color_scheme_reporting: false,
    }
    .plain_page_keys_use_host_scrollback());
}

fn text_cell(text: &str) -> crate::ghostty::ScreenTextCell {
    crate::ghostty::ScreenTextCell {
        wide: crate::ghostty::CellWide::Narrow,
        graphemes: text.chars().map(u32::from).collect(),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> crate::ghostty::RgbColor {
    crate::ghostty::RgbColor { r, g, b }
}

#[test]
fn dirty_full_collects_bounded_viewport_patch() {
    let mut terminal = crate::ghostty::Terminal::new(4, 3, 200).unwrap();
    terminal.write(b"one\r\ntwo\r\nthree");
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());

    let patch = match pane.collect_dirty_patch(4, 3) {
        TerminalDirtyPatchOutcome::Patch(patch) => patch,
        outcome => panic!("expected viewport patch, got {outcome:?}"),
    };

    assert_eq!(patch.rows.len(), 3);
    assert_eq!(
        patch.rows.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(patch.rows.iter().all(|(_, cells)| cells.len() == 4));
    assert!(matches!(
        pane.collect_dirty_patch(4, 3),
        TerminalDirtyPatchOutcome::Clean
    ));
}

#[test]
fn redefined_palette_entries_render_as_rgb_and_others_stay_indexed() {
    let default = [rgb(1, 2, 3); 256];
    let mut active = default;
    active[18] = rgb(169, 177, 214);
    let overrides = PaletteOverrides::new(&active, &default).expect("index 18 differs");

    assert_eq!(
        ghostty_cell_color(crate::ghostty::CellColor::Palette(18), Some(&overrides)),
        Color::Rgb(169, 177, 214)
    );
    // Untouched entries keep being forwarded, so they still follow the host theme.
    assert_eq!(
        ghostty_cell_color(crate::ghostty::CellColor::Palette(19), Some(&overrides)),
        Color::Indexed(19)
    );
    // ...and so does everything when the program never wrote a palette at all.
    assert_eq!(
        ghostty_cell_color(crate::ghostty::CellColor::Palette(18), None),
        Color::Indexed(18)
    );
}

#[test]
fn direct_rgb_cells_are_unaffected_by_palette_overrides() {
    let default = [rgb(1, 2, 3); 256];
    let mut active = default;
    active[18] = rgb(169, 177, 214);
    let overrides = PaletteOverrides::new(&active, &default).expect("index 18 differs");
    assert_eq!(
        ghostty_cell_color(
            crate::ghostty::CellColor::Rgb(rgb(122, 162, 247)),
            Some(&overrides)
        ),
        Color::Rgb(122, 162, 247)
    );
}

fn wide_text_cells(text: &str) -> [crate::ghostty::ScreenTextCell; 2] {
    [
        crate::ghostty::ScreenTextCell {
            wide: crate::ghostty::CellWide::Wide,
            graphemes: text.chars().map(u32::from).collect(),
        },
        crate::ghostty::ScreenTextCell {
            wide: crate::ghostty::CellWide::SpacerTail,
            graphemes: Vec::new(),
        },
    ]
}

fn text_row(
    cells: impl IntoIterator<Item = crate::ghostty::ScreenTextCell>,
    soft_wrapped: bool,
) -> crate::ghostty::ScreenTextRow {
    crate::ghostty::ScreenTextRow {
        cells: cells.into_iter().collect(),
        soft_wrapped,
        wrap_continuation: false,
    }
}

fn search_primary(
    buffer: &RetainedTextBuffer,
    query: &str,
    case_sensitive: bool,
) -> Vec<TerminalTextMatch> {
    buffer
        .search_window(
            query,
            case_sensitive,
            crate::ghostty::ActiveScreen::Primary,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            usize::MAX,
        )
        .matches
}

fn write_numbered_lines(terminal: &mut crate::ghostty::Terminal, count: usize) {
    for i in 0..count {
        terminal.write(format!("{i:06}\r\n").as_bytes());
    }
}

fn write_wrapped_contract_lines(terminal: &mut crate::ghostty::Terminal, count: usize) {
    for i in 0..count {
        terminal.write(format!("WRAP-{i:03}-abcdefghijklmnopqrstuvwxyz\r\n").as_bytes());
    }
    terminal.write(b"END");
}

#[test]
fn retained_text_specialized_build_modes_preserve_results_and_row_offsets() {
    let mut first = wide_text_cells("界").to_vec();
    first.extend(" ab.c ".chars().map(|ch| text_cell(&ch.to_string())));
    let mut second = vec![text_cell("e\u{301}")];
    second.extend(" d.e   ".chars().map(|ch| text_cell(&ch.to_string())));
    let rows = vec![
        text_row(first, true),
        text_row(second, false),
        text_row(
            "        ".chars().map(|ch| text_cell(&ch.to_string())),
            false,
        ),
        text_row(
            "tail    ".chars().map(|ch| text_cell(&ch.to_string())),
            false,
        ),
    ];
    let combined = RetainedTextBuffer::new(8, rows.clone());
    let row_offset = 7;

    for offset in [0, row_offset] {
        let search = RetainedTextBuffer::new_search(8, rows.clone(), offset);
        for query in ["界 a", "c e\u{301}", "d.e", "\u{301}", "tail", "TAIL"] {
            for case_sensitive in [false, true] {
                let mut expected = search_primary(&combined, query, case_sensitive);
                for found in &mut expected {
                    found.start.row += offset;
                    found.end.row += offset;
                }
                assert_eq!(
                    search_primary(&search, query, case_sensitive),
                    expected,
                    "query={query:?}, case_sensitive={case_sensitive}, offset={offset}"
                );
            }
        }

        let words = RetainedTextBuffer::new_words(8, rows.clone(), offset);
        for motion in [
            TerminalWordMotion::NextStart,
            TerminalWordMotion::PreviousStart,
            TerminalWordMotion::NextEnd,
            TerminalWordMotion::NextBigStart,
            TerminalWordMotion::PreviousBigStart,
            TerminalWordMotion::NextBigEnd,
        ] {
            for row in 0..4 {
                for col in 0..8 {
                    let expected =
                        combined
                            .word_motion(row, col, motion)
                            .map(|point| TerminalTextPoint {
                                row: point.row + offset,
                                col: point.col,
                            });
                    assert_eq!(
                        words.word_motion(row + offset, col, motion),
                        expected,
                        "row={row}, col={col}, motion={motion:?}, offset={offset}"
                    );
                }
            }
        }
    }
}

#[test]
fn retained_search_windows_preserve_navigation_limits_and_match_metadata() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let texts = ["hit", "HIT", "Hit", "hit", "hIt"];
    let buffer = RetainedTextBuffer::new_search(
        8,
        texts
            .iter()
            .map(|text| text_row(text.chars().map(|ch| text_cell(&ch.to_string())), false))
            .collect(),
        0,
    );
    let point = |row, col| TerminalTextPoint { row, col };
    let previous = |row| Some((point(row, 0), point(row, 2)));
    let cases = [
        (TerminalSearchDirection::Forward, point(0, 0), None, 1),
        (TerminalSearchDirection::Forward, point(2, 0), None, 3),
        (TerminalSearchDirection::Forward, point(4, 0), None, 0),
        (TerminalSearchDirection::Backward, point(0, 0), None, 4),
        (TerminalSearchDirection::Backward, point(2, 0), None, 1),
        (TerminalSearchDirection::Backward, point(4, 3), None, 4),
        (
            TerminalSearchDirection::Forward,
            point(0, 0),
            previous(1),
            2,
        ),
        (
            TerminalSearchDirection::Backward,
            point(4, 3),
            previous(3),
            2,
        ),
        (
            TerminalSearchDirection::Forward,
            point(2, 0),
            previous(4),
            0,
        ),
        (
            TerminalSearchDirection::Backward,
            point(2, 0),
            previous(0),
            4,
        ),
    ];
    // Expected window starts for each global target index, including edge clamping.
    let windows = [
        (0, [0, 0, 0, 0, 0]),
        (1, [0, 1, 2, 3, 4]),
        (2, [0, 0, 1, 2, 3]),
        (3, [0, 0, 1, 2, 2]),
        (4, [0, 0, 0, 1, 1]),
        (5, [0, 0, 0, 0, 0]),
        (8, [0, 0, 0, 0, 0]),
        (usize::MAX, [0, 0, 0, 0, 0]),
    ];

    for screen in [
        crate::ghostty::ActiveScreen::Primary,
        crate::ghostty::ActiveScreen::Alternate,
    ] {
        let expected_matches: Vec<_> = texts
            .iter()
            .enumerate()
            .map(|(row, text)| {
                let mut hasher = DefaultHasher::new();
                text.hash(&mut hasher);
                TerminalTextMatch {
                    start: point(row as u32, 0),
                    end: point(row as u32, 2),
                    source_fingerprint: hasher.finish(),
                    scan_cols: 8,
                    scan_screen: screen,
                }
            })
            .collect();
        for (direction, cursor, previous, target) in cases {
            for (limit, starts) in windows {
                let expected = if limit == 0 {
                    TerminalSearchWindow {
                        matches: Vec::new(),
                        current: None,
                        current_global: None,
                        total: 0,
                    }
                } else {
                    let start = starts[target];
                    let end = start + limit.min(texts.len());
                    TerminalSearchWindow {
                        matches: expected_matches[start..end].to_vec(),
                        current: Some(target - start),
                        current_global: Some(target),
                        total: texts.len(),
                    }
                };
                assert_eq!(
                    buffer.search_window("hit", false, screen, direction, cursor, previous, limit),
                    expected,
                    "direction={direction:?}, cursor={cursor:?}, previous={previous:?}, limit={limit}, screen={screen:?}"
                );
            }
        }
    }
}

#[test]
fn retained_search_empty_queries_and_missing_matches_have_empty_windows() {
    let buffer =
        RetainedTextBuffer::new_search(8, vec![text_row([text_cell("e\u{301}")], false)], 0);
    for query in ["", "absent", "e", "\u{301}"] {
        for direction in [
            TerminalSearchDirection::Forward,
            TerminalSearchDirection::Backward,
        ] {
            assert_eq!(
                buffer.search_window(
                    query,
                    true,
                    crate::ghostty::ActiveScreen::Primary,
                    direction,
                    TerminalTextPoint { row: 0, col: 0 },
                    None,
                    3,
                ),
                TerminalSearchWindow {
                    matches: Vec::new(),
                    current: None,
                    current_global: None,
                    total: 0,
                },
                "query={query:?}, direction={direction:?}"
            );
        }
    }
}

#[test]
fn retained_search_window_counts_only_complete_grapheme_matches() {
    let buffer = RetainedTextBuffer::new_search(
        8,
        vec![text_row(
            ["e\u{301}", " ", "e", " ", "e\u{301}", " ", "e"].map(text_cell),
            false,
        )],
        0,
    );
    for (direction, col, target, target_col) in [
        (TerminalSearchDirection::Forward, 0, 0, 2),
        (TerminalSearchDirection::Forward, 2, 1, 6),
        (TerminalSearchDirection::Forward, 6, 0, 2),
        (TerminalSearchDirection::Backward, 0, 1, 6),
        (TerminalSearchDirection::Backward, 6, 0, 2),
    ] {
        let window = buffer.search_window(
            "e",
            true,
            crate::ghostty::ActiveScreen::Primary,
            direction,
            TerminalTextPoint { row: 0, col },
            None,
            1,
        );
        assert_eq!(window.total, 2);
        assert_eq!(window.current, Some(0));
        assert_eq!(window.current_global, Some(target));
        assert_eq!(window.matches.len(), 1);
        assert_eq!(
            window.matches[0].start,
            TerminalTextPoint {
                row: 0,
                col: target_col
            }
        );
        assert_eq!(window.matches[0].end, window.matches[0].start);
    }
}

#[test]
fn retained_text_search_crosses_soft_wraps_but_not_hard_lines() {
    let buffer = RetainedTextBuffer::new(
        5,
        vec![
            text_row("abcde".chars().map(|ch| text_cell(&ch.to_string())), true),
            text_row("fgh  ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("abc  ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    let matches = search_primary(&buffer, "def", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 3 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 1, col: 0 });
    assert!(search_primary(&buffer, "hab", true).is_empty());
}

#[test]
fn retained_text_search_maps_wide_and_combining_graphemes_to_cells() {
    let mut cells = vec![text_cell("A")];
    cells.extend(wide_text_cells("界"));
    cells.push(text_cell("e\u{301}"));
    cells.push(text_cell("Z"));
    let buffer = RetainedTextBuffer::new(5, vec![text_row(cells, false)]);

    let matches = search_primary(&buffer, "界e\u{301}", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 1 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 0, col: 3 });
    assert!(search_primary(&buffer, "\u{301}", true).is_empty());
}

#[test]
fn retained_text_search_skips_wide_spacer_heads_at_soft_wraps() {
    let mut first = "abcd"
        .chars()
        .map(|ch| text_cell(&ch.to_string()))
        .collect::<Vec<_>>();
    first.push(crate::ghostty::ScreenTextCell {
        wide: crate::ghostty::CellWide::SpacerHead,
        graphemes: Vec::new(),
    });
    let mut second = wide_text_cells("界").to_vec();
    second.extend("xyz".chars().map(|ch| text_cell(&ch.to_string())));
    let buffer = RetainedTextBuffer::new(5, vec![text_row(first, true), text_row(second, false)]);

    let matches = search_primary(&buffer, "d界", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 3 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 1, col: 1 });
}

#[test]
fn retained_text_word_motion_does_not_split_at_a_wide_spacer_head() {
    let mut first = "abcd"
        .chars()
        .map(|ch| text_cell(&ch.to_string()))
        .collect::<Vec<_>>();
    first.push(crate::ghostty::ScreenTextCell {
        wide: crate::ghostty::CellWide::SpacerHead,
        graphemes: Vec::new(),
    });
    let mut second = wide_text_cells("界").to_vec();
    second.extend("xyz".chars().map(|ch| text_cell(&ch.to_string())));
    let buffer = RetainedTextBuffer::new(5, vec![text_row(first, true), text_row(second, false)]);

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        None
    );
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextEnd),
        Some(TerminalTextPoint { row: 1, col: 4 })
    );
}

#[test]
fn retained_text_search_is_literal_and_unicode_case_aware() {
    let buffer = RetainedTextBuffer::new(
        12,
        vec![text_row(
            "CAFÉ a.b    ".chars().map(|ch| text_cell(&ch.to_string())),
            false,
        )],
    );

    assert_eq!(search_primary(&buffer, "café", false).len(), 1);
    assert!(search_primary(&buffer, "café", true).is_empty());
    assert_eq!(search_primary(&buffer, "a.b", true).len(), 1);
    assert!(search_primary(&buffer, "a?b", true).is_empty());
}

#[test]
fn retained_text_word_motions_use_tmux_separators_across_rows() {
    let buffer = RetainedTextBuffer::new(
        6,
        vec![
            text_row("a_b.c ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("—d    ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
    assert_eq!(
        buffer.word_motion(0, 3, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 1, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(1, 1, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 1, col: 0 })
    );
}

#[test]
fn retained_text_big_word_motions_treat_only_whitespace_as_separators() {
    let buffer = RetainedTextBuffer::new(
        20,
        vec![text_row(
            "foo.bar baz qux/quux"
                .chars()
                .map(|ch| text_cell(&ch.to_string())),
            false,
        )],
    );

    // `W` skips punctuation-separated segments and lands on the next
    // whitespace-delimited run.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 0, col: 8 })
    );
    assert_eq!(
        buffer.word_motion(0, 8, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 0, col: 12 })
    );
    // `E` lands on the last character of the current/next run.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 6 })
    );
    assert_eq!(
        buffer.word_motion(0, 6, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 10 })
    );
    assert_eq!(
        buffer.word_motion(0, 12, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 19 })
    );
    // `B` returns to the beginning of the previous run.
    assert_eq!(
        buffer.word_motion(0, 19, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 12 })
    );
    assert_eq!(
        buffer.word_motion(0, 12, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 8 })
    );
    assert_eq!(
        buffer.word_motion(0, 8, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );

    // Lowercase motions keep their punctuation-aware behavior.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
    assert_eq!(
        buffer.word_motion(0, 3, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
}

#[test]
fn retained_text_big_word_motions_cross_rows_and_blank_lines() {
    let buffer = RetainedTextBuffer::new(
        6,
        vec![
            text_row("a.b-c ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("      ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("d_e   ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 2, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(2, 0, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 2, col: 2 })
    );
}

#[test]
fn live_terminal_word_motion_expands_across_long_blank_history() {
    let mut terminal = crate::ghostty::Terminal::new(10, 3, 200).unwrap();
    terminal.write(b"origin\r\n");
    for _ in 0..80 {
        terminal.write(b"\r\n");
    }
    let last_row = terminal.total_rows().unwrap().saturating_sub(1) as u32;
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());

    assert_eq!(
        pane.word_motion_target(last_row, 0, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );
}

#[test]
fn live_terminal_word_end_expands_through_a_long_soft_wrap() {
    let mut terminal = crate::ghostty::Terminal::new(2, 3, 200).unwrap();
    let word = "a".repeat(132);
    terminal.write(word.as_bytes());
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
    let text_match = pane
        .search_text_window(
            &word,
            true,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            1,
        )
        .matches[0];

    assert_eq!(
        pane.word_motion_target(
            text_match.start.row,
            text_match.start.col,
            TerminalWordMotion::NextEnd,
        ),
        Some(text_match.end)
    );
}

#[test]
fn backward_retained_search_finds_the_previous_occurrence() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    terminal.write(b"needle first\r\nneedle second\r\n");
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
    let found = pane.search_text_window(
        "needle",
        true,
        TerminalSearchDirection::Backward,
        TerminalTextPoint { row: 2, col: 0 },
        None,
        1,
    );
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].start, TerminalTextPoint { row: 1, col: 0 });
}

#[test]
fn live_terminal_word_end_expands_through_a_long_wide_soft_wrap() {
    let mut terminal = crate::ghostty::Terminal::new(2, 3, 200).unwrap();
    let word = "界".repeat(66);
    terminal.write(word.as_bytes());
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
    let text_match = pane
        .search_text_window(
            &word,
            true,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            1,
        )
        .matches[0];

    // The word end sits on the head cell of the final wide glyph, past the
    // initial read window, so the window has to expand to reach it.
    assert_eq!(
        pane.word_motion_target(
            text_match.start.row,
            text_match.start.col,
            TerminalWordMotion::NextEnd,
        ),
        Some(TerminalTextPoint {
            row: text_match.end.row,
            col: 0,
        })
    );
}

#[test]
#[ignore = "explicit sparse/dense render comparison against the full-cell reference"]
fn render_scale_profile_blank_density() {
    fn measure<const FAST: bool>(pane: &GhosttyPaneTerminal) -> f64 {
        let mut target =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        for _ in 0..20 {
            pane.render_cells::<FAST>(&mut target.get_frame(), Rect::new(0, 0, 80, 24), false);
        }
        let start = std::time::Instant::now();
        for _ in 0..500 {
            pane.render_cells::<FAST>(&mut target.get_frame(), Rect::new(0, 0, 80, 24), false);
        }
        start.elapsed().as_secs_f64() * 1e6 / 500.0
    }
    for percent in [0, 50, 100] {
        let pane =
            GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(80, 24, 100).unwrap()).unwrap();
        let text = "X".repeat(80 * percent / 100);
        for row in 1..=24 {
            pane.process_pty_bytes(
                PaneId::from_raw(1),
                format!("\x1b[{row};1H{text}").as_bytes(),
            );
        }
        let mut full = Vec::new();
        let mut fast = Vec::new();
        for _ in 0..3 {
            full.push(measure::<false>(&pane));
            fast.push(measure::<true>(&pane));
            fast.push(measure::<true>(&pane));
            full.push(measure::<false>(&pane));
        }
        full.sort_by(f64::total_cmp);
        fast.sort_by(f64::total_cmp);
        let full = (full[2] + full[3]) / 2.0;
        let fast = (fast[2] + fast[3]) / 2.0;
        eprintln!("blank_density percent={percent} reference_us={full:.2} fast_us={fast:.2} change_percent={:.2}",(fast/full-1.0)*100.0);
    }
}

#[test]
fn default_blank_fast_render_matches_full_cells_with_live_colors_and_styles() {
    fn draw<const FAST: bool>(pane: &GhosttyPaneTerminal) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(48, 8)).unwrap();
        let mut frame = terminal.get_frame();
        pane.render_cells::<FAST>(&mut frame, Rect::new(0, 0, 48, 8), false);
        frame.buffer_mut().clone()
    }
    for data in [
        "",
        "plain 中文\r\ntext",
        "\x1b[1;48H界\x1b[2;1Hwide-wrap",
        "\x1b[8minvisible\x1b[0m",
        "\x1b[?5hglobal inverse",
        "\x1b[1;3;4;7mstyled\x1b[0m\r\nnormal",
        "\x1b]10;#112233\x07\x1b]11;#445566\x07live defaults",
        "\x1b[44m\x1b[2K\r\nbg erase\x1b[0m",
        "\x1b]4;1;#aabbcc\x07\x1b[31mpalette\x1b[0m",
        "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
        "\x1b[?1049h\x1b[2Jalternate",
    ] {
        let pane =
            GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(48, 8, 100).unwrap()).unwrap();
        pane.process_pty_bytes(PaneId::from_raw(1), data.as_bytes());
        assert_eq!(draw::<true>(&pane), draw::<false>(&pane), "stream {data:?}");
    }
}

fn current_palette_color(pane: &GhosttyPaneTerminal, index: u8) -> crate::ghostty::RgbColor {
    let mut core = pane.core.lock().unwrap();
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = &mut *core;
    render_state.update(terminal).unwrap();
    render_state.colors().unwrap().palette[usize::from(index)]
}

fn expected_osc_rgb_response(command: &str, color: crate::ghostty::RgbColor) -> Bytes {
    let r = u16::from(color.r) * 257;
    let g = u16::from(color.g) * 257;
    let b = u16::from(color.b) * 257;
    Bytes::from(format!("\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}\x1b\\"))
}

#[test]
fn process_pty_bytes_reports_latest_libghostty_pwd_callback() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let partial = pane.process_pty_bytes(pane_id, b"\x1b]7;file:///tmp/herdr%20");
    assert_eq!(partial.reported_cwd, None);

    let completed = pane.process_pty_bytes(pane_id, b"repo\x07");
    assert_eq!(
        completed.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/herdr repo"))
    );

    let latest = pane.process_pty_bytes(
        pane_id,
        b"\x1b]9;9;/tmp/conemu\x1b\\\x1b]1337;CurrentDir=/tmp/iterm2\x1b\\",
    );
    assert_eq!(
        latest.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/iterm2"))
    );
}

#[test]
fn process_pty_bytes_surfaces_live_bells_only() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.seed_history_ansi("stale\x07");
    let result = pane.process_pty_bytes(pane_id, b"\x07\x1b]0;title\x07\x07");

    assert_eq!(result.terminal_bells, 2);
    let drained = pane.process_pty_bytes(pane_id, b"live output");
    assert_eq!(drained.terminal_bells, 0);
}

#[test]
fn process_pty_bytes_reports_only_completed_title_changes() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert!(
        !pane
            .process_pty_bytes(pane_id, b"\x1b]0;buil")
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, b"ding\x07")
            .terminal_title_changed
    );
    assert!(
        !pane
            .process_pty_bytes(pane_id, b"\x1b]2;building\x07")
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, b"\x1b]2;done\x07")
            .terminal_title_changed
    );
}

#[test]
fn process_pty_bytes_surfaces_clipboard_writes_without_other_results() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let result = pane.process_pty_bytes(PaneId::from_raw(1), b"output\x1b]52;c;Y2xpcGJvYXJk\x07");

    assert!(result.request_render);
    assert_eq!(result.render_delay, None);
    assert_eq!(result.terminal_bells, 0);
    assert_eq!(result.clipboard_writes, vec![b"clipboard".to_vec()]);
    assert_eq!(result.reported_cwd, None);
    assert!(result.terminal_responses.is_empty());
}

#[test]
fn seeded_history_clipboard_write_does_not_leak_into_live_output() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_history_ansi("\x1b]52;c;c3RhbGU=\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), b"live output");

    assert!(result.clipboard_writes.is_empty());
}

#[test]
fn seeded_history_pwd_does_not_leak_into_live_output() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_history_ansi("\x1b]7;file:///tmp/restored\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), b"live output");

    assert_eq!(result.reported_cwd, None);
}

fn expected_xtgettcap_response(cap_hex: &str, value: Option<&[u8]>) -> Bytes {
    let mut response = format!("\x1bP1+r{cap_hex}").into_bytes();
    if let Some(value) = value {
        response.push(b'=');
        append_upper_hex(value, &mut response);
    }
    response.extend_from_slice(b"\x1b\\");
    Bytes::from(response)
}

fn append_upper_hex(bytes: &[u8], output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}

#[test]
fn decscusr_cursor_shape_preserves_blinking_variants() {
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Block, true),
        1
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Block, false),
        2
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Underline, true),
        3
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Underline, false),
        4
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Bar, true),
        5
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::Bar, false),
        6
    );
    assert_eq!(
        decscusr_cursor_shape(crate::ghostty::CursorVisualStyle::BlockHollow, false),
        2
    );
}

#[test]
fn cursor_state_uses_terminal_default_until_child_sets_shape() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert_eq!(pane.cursor_state().unwrap().shape, 0);

    pane.process_pty_bytes(pane_id, b"\x1b[6 q");

    assert_eq!(pane.cursor_state().unwrap().shape, 6);
}

#[test]
fn cursor_state_returns_terminal_default_after_decscusr_reset() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"\x1b[2 q");
    assert_eq!(pane.cursor_state().unwrap().shape, 2);

    pane.process_pty_bytes(pane_id, b"\x1b[0 q");

    assert_eq!(pane.cursor_state().unwrap().shape, 0);
}

#[test]
fn cursor_shape_tracker_handles_split_decscusr_sequences() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"\x1b[");
    pane.process_pty_bytes(pane_id, b"5 ");
    pane.process_pty_bytes(pane_id, b"q");

    assert_eq!(pane.cursor_state().unwrap().shape, 5);
}

#[test]
fn cursor_state_uses_live_position_when_settle_policy_disabled() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"x");
    let result = pane.process_pty_bytes(pane_id, b"\x1b[6;21H");

    assert_eq!(result.render_delay, None);
    assert_eq!(
        pane.cursor_state()
            .map(|cursor| (cursor.x, cursor.y, cursor.visible)),
        Some((20, 5, true))
    );
}

#[test]
fn cursor_settle_policy_controls_render_delay() {
    let delay = Some(CURSOR_POSITION_SETTLE);
    assert_eq!(
        render_delay_after_pty_write(false, false, delay, true),
        delay
    );
    assert_eq!(
        render_delay_after_pty_write(false, false, delay, false),
        None
    );
    assert_eq!(
        render_delay_after_pty_write(false, true, delay, false),
        Some(KITTY_GRAPHICS_REDRAW_SETTLE)
    );
    assert_eq!(render_delay_after_pty_write(true, false, delay, true), None);
    let jump_delay = Some(Duration::from_millis(100));
    assert_eq!(
        render_delay_after_pty_write(false, true, jump_delay, true),
        jump_delay
    );
}

#[test]
fn host_terminal_theme_restore_probe_skips_when_no_transient_override() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_skips_when_host_theme_unknown() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.theme.transient_owner_pgid = Some(42);
    }
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_skips_on_alternate_screen() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1049h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.theme.transient_owner_pgid = Some(42);
        core.theme.host = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
    }
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_runs_when_restore_is_pending() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.theme.transient_owner_pgid = Some(42);
        core.theme.host = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
    }
    let core = pane.core.lock().unwrap();

    assert!(should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn ghostty_render_can_suppress_cursor_position() {
    let mut first_terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    first_terminal.write(b"left");
    let first = GhosttyPaneTerminal::new(first_terminal).unwrap();

    let mut second_terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    second_terminal.write(b"r\r\nb");
    let second = GhosttyPaneTerminal::new(second_terminal).unwrap();

    let backend = ratatui::backend::TestBackend::new(40, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            first.render(frame, Rect::new(0, 0, 20, 5), true);
            second.render(frame, Rect::new(20, 0, 20, 5), false);
        })
        .unwrap();

    terminal.backend_mut().assert_cursor_position((4, 0));
}

#[test]
fn ghostty_keyboard_protocol_tracks_live_terminal_flags() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert_eq!(
        pane.keyboard_protocol(),
        Some(crate::input::KeyboardProtocol::Kitty { flags: 3 })
    );
}

#[test]
fn ghostty_plain_text_chars_still_encode_as_text() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyModifiers::empty(),
    ));

    assert_eq!(encoded, b"a");
}

#[test]
fn ghostty_backtab_preserves_shift_across_keyboard_protocols() {
    for (kitty_flags, expected) in [
        (None, b"\x1b[Z".as_slice()),
        (Some(1), b"\x1b[9;2u".as_slice()),
    ] {
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        if let Some(flags) = kitty_flags {
            terminal.write(format!("\x1b[>{flags}u").as_bytes());
        }
        let pane = GhosttyPaneTerminal::new(terminal).unwrap();

        for modifiers in [
            crossterm::event::KeyModifiers::empty(),
            crossterm::event::KeyModifiers::SHIFT,
        ] {
            let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
                crossterm::event::KeyCode::BackTab,
                modifiers,
            ));
            assert_eq!(encoded, expected, "backtab with modifiers {modifiers:?}");
        }
    }

    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::empty(),
    ));
    assert_eq!(encoded, b"\t");
}

#[test]
fn ghostty_ctrl_tab_matches_the_pane_keyboard_protocol() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let legacy = GhosttyPaneTerminal::new(terminal).unwrap();
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::CONTROL,
    );

    assert_eq!(legacy.encode_terminal_key(key.clone()), b"\t");

    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let kitty = GhosttyPaneTerminal::new(terminal).unwrap();
    assert_eq!(kitty.encode_terminal_key(key), b"\x1b[9;5u");
}

#[test]
fn ghostty_legacy_modified_enter_is_shell_compatible() {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    for modifiers in [
        KeyModifiers::empty(),
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::SUPER,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::ALT | KeyModifiers::SHIFT,
        KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER,
    ] {
        let key = crate::input::TerminalKey::new(KeyCode::Enter, modifiers);
        // Super chords never reach a plain shell, so Cmd+Enter cannot run
        // the command line.
        let expected = if modifiers.contains(KeyModifiers::SUPER) {
            b"".as_slice()
        } else if modifiers.contains(KeyModifiers::ALT) {
            b"\x1b\r".as_slice()
        } else {
            b"\r".as_slice()
        };
        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            assert_eq!(
                pane.encode_terminal_key(key.clone().with_kind(kind)),
                expected,
                "{modifiers:?} {kind:?}"
            );
        }
        assert_eq!(
            pane.encode_terminal_key(key.clone().with_repeat_count(3)),
            expected.repeat(3),
            "{modifiers:?} grouped repeat"
        );
        assert!(
            pane.encode_terminal_key(key.with_kind(KeyEventKind::Release))
                .is_empty(),
            "{modifiers:?} release"
        );
    }
}

#[test]
fn ghostty_modified_enter_tracks_live_protocol_negotiation() {
    use crossterm::event::{KeyCode, KeyModifiers};

    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    // Like Ghostty, modifyOtherKeys level 1 behaves as if nothing was negotiated.
    // Super+Enter is not forwarded to panes without a keyboard protocol.
    let legacy = ["\r", "\r", "", "\x1b\r"];
    let mode_two = [
        "\x1b[27;2;13~",
        "\x1b[27;5;13~",
        "\x1b[27;9;13~",
        "\x1b[27;3;13~",
    ];
    let kitty = ["\x1b[13;2u", "\x1b[13;5u", "\x1b[13;9u", "\x1b[13;3u"];

    for (sequence, expected) in [
        ("", legacy),
        ("\x1b[>4;1m", legacy),
        ("\x1b[>4;2m", mode_two),
        ("\x1b[>4n", legacy),
        ("\x1b[>4;2m", mode_two),
        ("\x1b[>4;0m", legacy),
        ("\x1b[>5u", kitty),
        ("\x1b[<u", legacy),
        ("\x1b[>4;2m\x1b[>1u", kitty),
        ("\x1b[<u", mode_two),
        ("\x1b[>4;0m", legacy),
        ("\x1b[>4;1m", legacy),
        ("\x1b[>04n", legacy),
        ("\x1b[>4;2m", mode_two),
        ("\x1b[>4", mode_two),
        ("n", legacy),
    ] {
        pane.process_pty_bytes(pane_id, sequence.as_bytes());
        for (modifiers, expected) in [
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
            KeyModifiers::ALT,
        ]
        .into_iter()
        .zip(expected)
        {
            let key = crate::input::TerminalKey::new(KeyCode::Enter, modifiers);
            assert_eq!(
                pane.encode_terminal_key(key),
                expected.as_bytes(),
                "{modifiers:?} after {sequence:?}"
            );
        }
    }
}

#[test]
fn ghostty_modified_enter_respects_existing_terminal_mode() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>4;2m");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::SHIFT,
    );

    assert_eq!(pane.encode_terminal_key(key), b"\x1b[27;2;13~");
}

#[test]
fn ghostty_enter_backspace_release_in_legacy_pane_emits_nothing() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    for code in [
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyCode::Backspace,
    ] {
        let press = pane.encode_terminal_key(crate::input::TerminalKey::new(
            code,
            crossterm::event::KeyModifiers::empty(),
        ));
        let release = pane.encode_terminal_key(
            crate::input::TerminalKey::new(code, crossterm::event::KeyModifiers::empty())
                .with_kind(crossterm::event::KeyEventKind::Release),
        );
        assert!(!press.is_empty(), "{code:?} press should emit bytes");
        assert!(
            release.is_empty(),
            "{code:?} release should emit nothing in a legacy pane, got {release:?}"
        );
    }
}

#[test]
fn ghostty_report_event_pane_keeps_basic_compatibility_keys_legacy() {
    // Push kitty flags including REPORT_EVENT_TYPES (0b10) + DISAMBIGUATE (0b1).
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    for (code, expected) in [
        (crossterm::event::KeyCode::Enter, b"\r".as_slice()),
        (crossterm::event::KeyCode::Backspace, b"\x7f".as_slice()),
    ] {
        let press = pane.encode_terminal_key(crate::input::TerminalKey::new(
            code,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(
            press, expected,
            "{code:?} press should stay legacy-compatible without REPORT_ALL_KEYS"
        );

        let release = pane.encode_terminal_key(
            crate::input::TerminalKey::new(code, crossterm::event::KeyModifiers::empty())
                .with_kind(crossterm::event::KeyEventKind::Release),
        );
        assert!(
            release.is_empty(),
            "{code:?} release should not fall back to legacy bytes, got {release:?}"
        );
    }
}

#[test]
fn ghostty_alt_letters_keep_esc_prefix_on_every_platform() {
    // libghostty treats macOS Option as a text modifier unless told otherwise;
    // Herdr has already decoded Alt from these host bytes.
    for (host_bytes, expected) in [
        ("\x1bf", &b"\x1bf"[..]),
        ("\x1bb", b"\x1bb"),
        ("\x1bA", b"\x1bA"),
        ("\x1b.", b"\x1b."),
    ] {
        let key = crate::input::parse_terminal_key_sequence(host_bytes).expect("alt key");
        assert_eq!(
            test_encode_key_for_app(b"", key),
            expected,
            "{host_bytes:?}"
        );
    }
}

#[test]
fn hyper_and_meta_chords_are_never_typed_as_their_bare_key() {
    // iTerm2 can report Ctrl as Meta (#4476): `CSI 97;33u`. Typing "a"
    // would run a command in vim normal mode; libghostty has no Meta.
    for host_bytes in ["\x1b[97;33u", "\x1b[120;17u"] {
        let key = crate::input::parse_terminal_key_sequence(host_bytes).expect("kitty key");
        for app_output in [&b""[..], b"\x1b[>1u", b"\x1b[>31u"] {
            assert!(
                test_encode_key_for_app(app_output, key.clone()).is_empty(),
                "{host_bytes:?} after {app_output:?}"
            );
        }
    }
}

#[test]
fn super_chords_reach_only_panes_that_negotiated_a_keyboard_protocol() {
    // Super+Space from a Kitty host (#4356) and Cmd+C (#3710).
    // Super+Enter must not become a plain Enter that runs the command line.
    for (host_bytes, kitty) in [
        ("\x1b[32;9u", &b"\x1b[32;9u"[..]),
        ("\x1b[99;9u", b"\x1b[99;9u"),
        ("\x1b[13;9u", b"\x1b[13;9u"),
    ] {
        let key = crate::input::parse_terminal_key_sequence(host_bytes).expect("kitty key");
        assert!(
            test_encode_key_for_app(b"", key.clone()).is_empty(),
            "{host_bytes:?} in a plain shell"
        );
        assert_eq!(
            test_encode_key_for_app(b"\x1b[>1u", key),
            kitty,
            "{host_bytes:?} in a Kitty pane"
        );
    }
}

#[test]
fn ctrl_shift_letters_reach_plain_shells_as_control_bytes() {
    // Lab capture: Alacritty and WezTerm (with or without Kitty keyboard)
    // send 0x01 for Ctrl+Shift+A; Ghostty's CSI u prints as garbage in a shell.
    for (host_bytes, plain, kitty) in [
        ("\x1b[97;6u", &b"\x01"[..], &b"\x1b[97;6u"[..]),
        ("\x1b[65;5u", b"\x01", b"\x1b[97;6u"),
        ("\x1b[97;8u", b"\x1b\x01", b"\x1b[97;8u"),
    ] {
        let key = crate::input::parse_terminal_key_sequence(host_bytes).expect("kitty key");
        assert_eq!(
            test_encode_key_for_app(b"", key.clone()),
            plain,
            "{host_bytes:?} in a plain shell"
        );
        assert_eq!(
            test_encode_key_for_app(b"\x1b[>1u", key),
            kitty,
            "{host_bytes:?} in a Kitty pane"
        );
    }
}

#[test]
fn ctrl_on_a_russian_layout_interrupts_a_plain_shell() {
    // Lab capture: kitty reports Ctrl+\u{441} as `CSI 1089::99;5u`; kitty
    // itself sends ^C to a plain shell.
    let key = crate::input::parse_terminal_key_sequence("\x1b[1089::99;5u").expect("key");
    assert_eq!(test_encode_key_for_app(b"", key.clone()), b"\x03");
    assert_eq!(test_encode_key_for_app(b"\x1b[>1u", key), b"\x1b[99;5u");
}

#[test]
fn legacy_alt_prefixes_the_produced_non_ascii_text() {
    // A legacy host's Alt+Shift+\u{f6}. libghostty on macOS would prefix the
    // unshifted "\u{f6}" instead.
    for host_bytes in ["\x1b\u{d6}", "\x1b\u{f6}"] {
        let key = crate::input::parse_terminal_key_sequence(host_bytes).expect("alt key");
        assert_eq!(
            test_encode_key_for_app(b"", key),
            host_bytes.as_bytes(),
            "{host_bytes:?}"
        );
    }
}

#[test]
fn ghostty_kitty_reports_keep_non_us_key_identity() {
    // Ctrl++ on a German layout, where + is an unshifted key. The report
    // already names the key; nothing may reinterpret it as US Shift+=.
    let key = crate::input::parse_terminal_key_sequence("\x1b[43;5u").expect("kitty key");
    assert_eq!(test_encode_key_for_app(b"\x1b[>1u", key), b"\x1b[43;5u");
}

#[test]
fn ghostty_disambiguate_reports_ctrl_letters_as_kitty_keys() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>1u");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyModifiers::CONTROL,
    ));

    assert_eq!(encoded, b"\x1b[97;5u");
}

#[test]
fn ghostty_encodes_text_keys_with_layout_text() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    for (key, expected) in [
        (
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Char('D'),
                crossterm::event::KeyModifiers::SHIFT,
            ),
            "D",
        ),
        (
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Char('%'),
                crossterm::event::KeyModifiers::empty(),
            ),
            "%",
        ),
        (
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Char('ö'),
                crossterm::event::KeyModifiers::empty(),
            )
            .with_generated_text(Some("ö".to_owned())),
            "ö",
        ),
        (
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Char('8'),
                crossterm::event::KeyModifiers::CONTROL,
            ),
            "\x7f",
        ),
    ] {
        assert_eq!(
            pane.encode_terminal_key(key.clone()),
            expected.as_bytes(),
            "{key:?}"
        );
    }
}

#[test]
fn ghostty_key_encoding_honors_application_cursor_mode() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal
        .mode_set(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS, true)
        .unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));

    assert_eq!(encoded, b"\x1bOA");
}

#[test]
fn ghostty_seed_handoff_input_state_restores_input_modes() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    pane.seed_handoff_input_state(InputState {
        alternate_screen: true,
        application_cursor: true,
        bracketed_paste: true,
        focus_reporting: true,
        mouse_protocol_mode: crate::input::MouseProtocolMode::ButtonMotion,
        mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Sgr,
        mouse_alternate_scroll: true,
        modify_other_keys: true,
        color_scheme_reporting: true,
    });

    assert_eq!(
        pane.input_state(),
        Some(InputState {
            alternate_screen: true,
            application_cursor: true,
            bracketed_paste: true,
            focus_reporting: true,
            mouse_protocol_mode: crate::input::MouseProtocolMode::ButtonMotion,
            mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Sgr,
            mouse_alternate_scroll: true,
            modify_other_keys: true,
            color_scheme_reporting: true,
        })
    );
    assert_eq!(pane.modify_other_keys_level(), 2);

    let encoded = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));
    assert_eq!(encoded, b"\x1bOA");

    let key = crate::input::parse_terminal_key_sequence("\x1b[13;2u").unwrap();
    let encoded = pane.encode_terminal_key(key.clone());
    assert_eq!(encoded, b"\x1b[27;2;13~");

    let encoded = pane.encode_mouse_wheel(
        crossterm::event::MouseEventKind::ScrollUp,
        crate::input::mouse::Position::Cell { column: 11, row: 9 },
        crossterm::event::KeyModifiers::empty(),
    );
    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<64;12;10M"[..]));
}

#[test]
fn grouped_key_repeats_expand_at_the_destination() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::empty(),
    )
    .with_repeat_count(3);

    assert_eq!(pane.encode_terminal_key(key), b"xxx");

    let shifted = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('/'),
        crossterm::event::KeyModifiers::SHIFT,
    )
    .with_generated_text(Some("/".to_owned()))
    .with_windows_record(crate::input::WindowsKeyRecord {
        key_down: true,
        repeat_count: 3,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: u16::from(b'/'),
        control_key_state: 0x0010,
    });
    let legacy_expected = b"///".as_slice();
    assert_eq!(pane.encode_terminal_key(shifted.clone()), legacy_expected);
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>15u");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    assert_eq!(
        pane.encode_terminal_key(shifted),
        b"\x1b[47;2u\x1b[47;2:2u\x1b[47;2:2u"
    );
}

#[test]
fn grouped_release_is_encoded_once() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>11u");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let release = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    )
    .with_kind(crossterm::event::KeyEventKind::Release);
    let expected = pane.encode_terminal_key(release.clone());

    assert!(!expected.is_empty());
    let mut malformed_release = release;
    malformed_release.repeat_count = 3;
    assert_eq!(pane.encode_terminal_key(malformed_release), expected);
}

#[test]
fn ghostty_key_encoder_updates_after_terminal_mode_changes() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let before = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));
    assert_eq!(before, b"\x1b[A");

    pane.process_pty_bytes(pane_id, b"\x1b[?1h");

    let after = pane.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));
    assert_eq!(after, b"\x1bOA");
}

#[test]
fn ghostty_key_encoder_updates_after_kitty_flag_changes() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::SHIFT,
    );

    let before = pane.encode_terminal_key(key.clone());
    pane.process_pty_bytes(pane_id, b"\x1b[>1u");
    let after = pane.encode_terminal_key(key.clone());

    assert_ne!(before, after);
    assert_eq!(after, b"\x1b[13;6u");
}

#[test]
fn ghostty_kitty_pane_encodes_shift_enter_as_csi_u() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, b"\x1b[>5u");

    let key = crate::input::parse_terminal_key_sequence("\x1b[13;2u").unwrap();
    let encoded = pane.encode_terminal_key(key.clone());

    assert_eq!(
        pane.keyboard_protocol(),
        Some(crate::input::KeyboardProtocol::Kitty { flags: 5 })
    );
    assert_eq!(encoded, b"\x1b[13;2u");
}

#[test]
fn ghostty_seed_keyboard_protocol_flags_restores_shift_enter_encoding() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_keyboard_protocol_flags(5);

    let key = crate::input::parse_terminal_key_sequence("\x1b[13;2u").unwrap();
    let encoded = pane.encode_terminal_key(key.clone());

    assert_eq!(
        pane.keyboard_protocol(),
        Some(crate::input::KeyboardProtocol::Kitty { flags: 5 })
    );
    assert_eq!(encoded, b"\x1b[13;2u");
}

#[test]
fn ghostty_keyboard_protocol_state_replays_nested_stack() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, b"\x1b[>1u\x1b[>5u");

    let ansi = pane.kitty_keyboard_state_ansi().unwrap();

    let restored_terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let restored = GhosttyPaneTerminal::new(restored_terminal).unwrap();
    restored.seed_keyboard_protocol_ansi(&ansi);
    assert_eq!(
        restored.keyboard_protocol(),
        Some(crate::input::KeyboardProtocol::Kitty { flags: 5 })
    );

    restored.process_pty_bytes(pane_id, b"\x1b[<u");
    assert_eq!(
        restored.keyboard_protocol(),
        Some(crate::input::KeyboardProtocol::Kitty { flags: 1 })
    );
}

#[test]
fn ghostty_modify_other_keys_mode_one_is_treated_as_unnegotiated() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let key = crate::input::parse_terminal_key_sequence("\x1b[13;2u").unwrap();

    pane.seed_history_ansi("\x1b[>4;1m");
    assert_eq!(pane.modify_other_keys_level(), 0);
    let encoded = pane.encode_terminal_key(key.clone());

    // Level 1 leaves legacy Enter encoding unchanged.
    assert_eq!(encoded, b"\r");
}

#[test]
fn ghostty_kitty_pane_encodes_parsed_legacy_alt_backspace_as_csi_u() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, b"\x1b[>1u");

    let key = crate::input::parse_terminal_key_sequence("\x1b\x7f").unwrap();
    let encoded = pane.encode_terminal_key(key.clone());

    assert_eq!(encoded, b"\x1b[127;3u");
}

#[test]
fn ghostty_kitty_pane_preserves_legacy_ctrl_alt_letter() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, b"\x1b[>5u");

    let mut events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b\x06");
    let crate::raw_input::RawInputEvent::Key(key) = events.remove(0) else {
        panic!("expected key event");
    };
    let encoded = pane.encode_terminal_key(key);

    assert_eq!(encoded, b"\x1b[102;7u");
}

#[test]
fn ghostty_pane_characterizes_ctrl_backspace_encoding() {
    let legacy =
        GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(80, 24, 0).unwrap()).unwrap();

    let ctrl_backspace = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(legacy.encode_terminal_key(ctrl_backspace.clone()), b"\x08");

    let plain_backspace = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::empty(),
    );
    assert_eq!(legacy.encode_terminal_key(plain_backspace), b"\x7f");

    let kitty =
        GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(80, 24, 0).unwrap()).unwrap();
    let pane_id = PaneId::from_raw(1);
    kitty.process_pty_bytes(pane_id, b"\x1b[>1u");

    assert_eq!(kitty.encode_terminal_key(ctrl_backspace), b"\x1b[127;5u");
}

#[test]
fn ghostty_key_encoders_are_isolated_per_pane() {
    let first =
        GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(80, 24, 0).unwrap()).unwrap();
    let second =
        GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(80, 24, 0).unwrap()).unwrap();

    first.process_pty_bytes(PaneId::from_raw(1), b"\x1b[?1h");

    let first_encoded = first.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));
    let second_encoded = second.encode_terminal_key(crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    ));

    assert_eq!(first_encoded, b"\x1bOA");
    assert_eq!(second_encoded, b"\x1b[A");
}

#[test]
fn wheel_routing_follows_tracking_screen_and_alternate_scroll_modes() {
    use crate::pane::WheelRouting;

    for mouse_mode in [None, Some(9), Some(1000), Some(1002), Some(1003)] {
        for alternate_screen in [false, true] {
            for alternate_scroll in [false, true] {
                let mut terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
                if alternate_screen {
                    terminal.write(b"\x1b[?1049h");
                }
                terminal.write(if alternate_scroll {
                    b"\x1b[?1007h"
                } else {
                    b"\x1b[?1007l"
                });
                if let Some(mode) = mouse_mode {
                    terminal.write(format!("\x1b[?{mode}h").as_bytes());
                }
                let reporting_from_modes = [9, 1000, 1002, 1003]
                    .into_iter()
                    .any(|mode| terminal.mode_get(mode).unwrap());
                assert_eq!(
                    terminal.mouse_tracking_enabled().unwrap(),
                    reporting_from_modes
                );
                assert_eq!(reporting_from_modes, mouse_mode.is_some());

                let fallback = if alternate_screen && alternate_scroll {
                    WheelRouting::AlternateScroll
                } else {
                    WheelRouting::HostScroll
                };
                let expected = if mouse_mode.is_some() {
                    WheelRouting::MouseReport
                } else {
                    fallback
                };
                let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
                assert_eq!(pane.wheel_routing(), Some(expected),
                    "mouse_mode={mouse_mode:?}, alternate_screen={alternate_screen}, alternate_scroll={alternate_scroll}");
                assert_eq!(pane.mouse_reporting_enabled(), mouse_mode.is_some());

                if let Some(mode) = mouse_mode {
                    pane.process_pty_bytes(
                        PaneId::from_raw(1),
                        format!("\x1b[?{mode}l").as_bytes(),
                    );
                    assert_eq!(pane.wheel_routing(), Some(fallback));
                    assert!(!pane.mouse_reporting_enabled());
                }
            }
        }
    }
}

#[test]
fn wheel_routing_mouse_encoding_alone_does_not_enable_tracking() {
    use crate::pane::WheelRouting;

    for encoding in [1005, 1006, 1016] {
        for alternate_screen in [false, true] {
            let mut terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
            if alternate_screen {
                terminal.write(b"\x1b[?1049h");
            }
            terminal.write(format!("\x1b[?1007h\x1b[?{encoding}h").as_bytes());
            let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal).unwrap());
            assert!(!pane.mouse_reporting_enabled());
            assert_eq!(
                pane.wheel_routing(),
                Some(if alternate_screen {
                    WheelRouting::AlternateScroll
                } else {
                    WheelRouting::HostScroll
                })
            );
        }
    }
}

#[test]
fn ghostty_mouse_button_encoding_uses_live_terminal_state() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1000h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        crate::input::mouse::Position::Cell { column: 11, row: 9 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<0;12;10m"[..]));
}

#[test]
fn ghostty_mouse_drag_encoding_uses_motion_reporting_state() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1002h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        crate::input::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::SHIFT,
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<36;5;7M"[..]));
}

#[test]
fn ghostty_mouse_drag_without_motion_reporting_is_not_forwarded() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1000h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        crate::input::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded, None);
}

#[test]
fn ghostty_mouse_moved_encoding_uses_any_motion_state() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1003h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let encoded = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::input::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<35;5;7M"[..]));
}

#[test]
fn ghostty_mouse_sgr_pixels_preserves_exact_and_scales_cell_input() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.resize(80, 24, 10, 20).unwrap();
    terminal.write(b"\x1b[?1003h\x1b[?1006h\x1b[?1016h");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let exact = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::input::mouse::Position::Pixels { x: 48, y: 139 },
        crossterm::event::KeyModifiers::empty(),
    );
    let fallback = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::input::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(exact.as_deref(), Some(&b"\x1b[<35;48;139M"[..]));
    assert_eq!(fallback.as_deref(), Some(&b"\x1b[<35;45;130M"[..]));
}

#[test]
fn ghostty_normalize_buffer_symbol_prefers_grapheme_width_when_metadata_disagrees() {
    const WIDE_GRAPHEME: &str = "🙂";
    const FLAG_GRAPHEME: &str = "🇧🇷";
    const FAMILY_GRAPHEME: &str = "👨‍👩‍👧";
    const VS16_GRAPHEME: &str = "⚠️";
    const EMOJI_GRAPHEME: &str = "💳";

    assert_eq!(
        ghostty_normalize_buffer_symbol(WIDE_GRAPHEME, crate::ghostty::CellWide::Wide),
        WIDE_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("a", crate::ghostty::CellWide::Wide),
        "  "
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(FLAG_GRAPHEME, crate::ghostty::CellWide::Wide),
        FLAG_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(FAMILY_GRAPHEME, crate::ghostty::CellWide::Wide),
        FAMILY_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("⌨️", crate::ghostty::CellWide::Narrow),
        "⌨️"
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(VS16_GRAPHEME, crate::ghostty::CellWide::Narrow),
        VS16_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(EMOJI_GRAPHEME, crate::ghostty::CellWide::Narrow),
        EMOJI_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(" ", crate::ghostty::CellWide::SpacerTail),
        ""
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("xx", crate::ghostty::CellWide::SpacerHead),
        " "
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("ｶ\u{ff9e}", crate::ghostty::CellWide::Wide),
        "ｶ\u{ff9e}"
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("ﾊ\u{ff9f}", crate::ghostty::CellWide::Wide),
        "ﾊ\u{ff9f}"
    );
}

fn render_cells_to_symbols(
    terminal: &mut crate::ghostty::Terminal,
) -> Vec<(crate::ghostty::CellWide, String)> {
    let mut render_state = crate::ghostty::RenderState::new().unwrap();
    render_state.update(terminal).unwrap();

    let mut row_iterator = crate::ghostty::RowIterator::new().unwrap();
    let mut rows = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    let mut row_cells = crate::ghostty::RowCells::new().unwrap();
    let mut grapheme_bytes = Vec::new();
    let mut symbol_scratch = String::new();
    let mut out = Vec::new();

    if rows.next() {
        let mut cells = rows.populate_cells(&mut row_cells).unwrap();
        while cells.next() {
            let wide = cells.wide().unwrap_or(crate::ghostty::CellWide::Narrow);
            let symbol = ghostty_buffer_symbol_into(
                &cells,
                wide,
                false,
                &mut grapheme_bytes,
                &mut symbol_scratch,
            )
            .unwrap()
            .to_string();
            out.push((wide, symbol));
        }
    }

    out
}

#[test]
fn grapheme_cluster_mode_renders_flag_emoji_in_single_wide_cell() {
    let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
    terminal.write("🇧🇷".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);

    assert!(
        cells
            .iter()
            .any(|(wide, symbol)| *wide == crate::ghostty::CellWide::Wide && symbol == "🇧🇷"),
        "expected a wide cell containing the full flag grapheme, got {cells:?}"
    );
}

#[test]
fn grapheme_cluster_mode_renders_zwj_family_in_single_wide_cell() {
    let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
    terminal.write("👨\u{200d}👩\u{200d}👧".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);

    assert!(
        cells
            .iter()
            .any(|(wide, symbol)| *wide == crate::ghostty::CellWide::Wide
                && symbol == "👨\u{200d}👩\u{200d}👧"),
        "expected a wide cell containing the full ZWJ grapheme, got {cells:?}"
    );
}

#[test]
fn halfwidth_katakana_voiced_marks_render() {
    let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
    terminal.write("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);
    let rendered: String = cells.iter().map(|(_, symbol)| symbol.as_str()).collect();

    assert!(
        rendered.contains("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ"),
        "expected halfwidth katakana with voiced marks to survive, got {cells:?}"
    );
}

#[test]
fn render_keeps_halfwidth_katakana_voiced_tail_empty() {
    let mut terminal = crate::ghostty::Terminal::new(20, 1, 0).unwrap();
    terminal.write("ｶﾞZ".as_bytes());
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let backend = ratatui::backend::TestBackend::new(20, 1);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 1), false))
        .unwrap();
    let buffer = terminal.backend().buffer();

    assert_eq!(buffer[(0, 0)].symbol(), "ｶ\u{ff9e}");
    assert_eq!(
        buffer[(1, 0)].symbol(),
        "",
        "wide spacer tail must stay empty so the host terminal does not overwrite the voiced kana"
    );
    assert_eq!(buffer[(2, 0)].symbol(), "Z");
}

#[test]
fn pane_scrollback_controls_round_trip_and_clamp_without_ui_interference() {
    let mut terminal = crate::ghostty::Terminal::new(80, 3, 100).unwrap();
    write_numbered_lines(&mut terminal, 1000);
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let before = pane.scroll_metrics().expect("scroll metrics before scroll");
    assert!(before.max_offset_from_bottom > 0);
    assert_eq!(before.offset_from_bottom, 0);

    for offset in [
        0,
        before.max_offset_from_bottom / 2,
        before.max_offset_from_bottom,
        usize::MAX,
    ] {
        pane.set_scroll_offset_from_bottom(offset);
        let after = pane.scroll_metrics().expect("scroll metrics after scroll");
        assert_eq!(
            after.offset_from_bottom,
            offset.min(after.max_offset_from_bottom)
        );
    }

    assert!(pane.visible_text().contains("000000"));
}

#[test]
fn empty_or_short_resize_keeps_following_bottom_when_output_creates_scrollback() {
    for initial in [b"".as_slice(), b"seed\r\n".as_slice()] {
        let mut terminal = crate::ghostty::Terminal::new(10, 3, 100).unwrap();
        terminal.write(initial);
        let pane = GhosttyPaneTerminal::new(terminal).unwrap();
        let pane_id = PaneId::from_raw(1);

        pane.resize(3, 10, 0, 0);
        pane.process_pty_bytes(pane_id, b"000000\r\n000001\r\n000002\r\n000003\r\n000004");

        let metrics = pane.scroll_metrics().expect("scroll metrics after output");
        assert_eq!(metrics.offset_from_bottom, 0);
        assert!(pane.visible_text().contains("000004"));
    }
}

#[test]
fn resize_that_removes_scrollback_restores_live_follow() {
    let mut terminal = crate::ghostty::Terminal::new(10, 3, 100).unwrap();
    terminal.write(b"000000\r\n000001\r\n000002\r\n000003\r\n000004");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.set_scroll_offset_from_bottom(1);
    pane.resize(5, 10, 0, 0);
    let resized = pane.scroll_metrics().expect("scroll metrics after resize");
    assert_eq!(resized.max_offset_from_bottom, 0);

    pane.process_pty_bytes(pane_id, b"\r\n000005\r\n000006");

    let metrics = pane.scroll_metrics().expect("scroll metrics after output");
    assert_eq!(metrics.offset_from_bottom, 0);
    assert!(pane.visible_text().contains("000006"));
}

#[test]
fn detection_text_reuse_preserves_text_and_requires_non_whitespace() {
    for (text, reusable) in [
        ("", false),
        (" \t\r\n", false),
        ("\u{2003}\n", false),
        ("hello\n", true),
        ("  界 e\u{301} \n", true),
    ] {
        assert_eq!(
            finish_detection_text(text.to_string()),
            (text.to_string(), reusable)
        );
    }
}

#[test]
fn detection_text_stays_at_bottom_when_viewport_is_scrolled() {
    let mut terminal = crate::ghostty::Terminal::new(80, 3, 100).unwrap();
    write_numbered_lines(&mut terminal, 10);
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let bottom_snapshot = pane.detection_text();
    assert_eq!(bottom_snapshot, pane.recent_text(3));
    assert!(bottom_snapshot.contains("000009"));

    let before = pane.scroll_metrics().expect("scroll metrics before scroll");
    pane.set_scroll_offset_from_bottom(before.max_offset_from_bottom);

    assert!(pane.visible_text().contains("000000"));
    assert_eq!(pane.detection_text(), bottom_snapshot);
}

#[test]
fn extract_selection_reads_screen_rows_not_current_viewport() {
    let mut terminal = crate::ghostty::Terminal::new(8, 3, 1024).unwrap();
    write_numbered_lines(&mut terminal, 8);
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    pane.set_scroll_offset_from_bottom(3);
    let metrics = pane
        .scroll_metrics()
        .expect("scroll metrics after initial scroll");
    let mut selection =
        crate::selection::Selection::anchor(PaneId::from_raw(1), 0, 0, Some(metrics));
    selection.drag(5, 2, Rect::new(0, 0, 8, 3), Some(metrics));

    pane.scroll_reset();

    let text = pane
        .extract_selection(&selection)
        .expect("selection should extract text");
    assert_eq!(text, "000003\n000004\n000005");
}

#[test]
fn recent_reads_include_viewport_before_scrollback_exists() {
    let mut terminal = crate::ghostty::Terminal::new(20, 20, 10_000_000).unwrap();
    terminal.write(b"hello123");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert_eq!(pane.recent_text(3), "hello123\n");
    assert_eq!(pane.recent_unwrapped_text(3), "hello123");
}

#[test]
fn alternate_screen_recent_reads_keep_physical_row_ranges() {
    let mut terminal = crate::ghostty::Terminal::new(20, 20, 100).unwrap();
    terminal.write(b"\x1b[?1049hhello123");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert_eq!(pane.recent_text(3), "");
    assert_eq!(pane.recent_unwrapped_text(3), "");
}

#[test]
fn recent_unwrapped_text_ignores_soft_wraps() {
    let mut terminal = crate::ghostty::Terminal::new(5, 3, 100).unwrap();
    terminal.write(b"ABCDEFGHIJ");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert_eq!(pane.recent_text(3), "ABCDE\nFGHIJ\n");
    assert_eq!(pane.recent_unwrapped_text(3), "ABCDEFGHIJ");
}

#[test]
fn recent_snapshots_report_omitted_rendered_rows() {
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"one\r\ntwo\r\nthree\r\nfour");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert!(pane.recent_text_snapshot(2).truncated);
    assert!(pane.recent_ansi_snapshot(2).truncated);
    assert!(pane.recent_unwrapped_text_snapshot(2).truncated);
    assert!(pane.recent_unwrapped_ansi_snapshot(2).truncated);
    assert!(!pane.recent_text_snapshot(100).truncated);
}

#[test]
fn plain_text_reads_skip_wide_character_spacer_cells() {
    let mut terminal = crate::ghostty::Terminal::new(40, 3, 100).unwrap();
    terminal.write("日本語テスト ABC 123".as_bytes());
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert_eq!(pane.visible_text(), "日本語テスト ABC 123\n");
    assert_eq!(pane.recent_text(3), "日本語テスト ABC 123\n");
    assert_eq!(pane.recent_unwrapped_text(3), "日本語テスト ABC 123");
    assert_eq!(pane.detection_text(), "日本語テスト ABC 123\n");
}

#[test]
fn recent_rows_preserve_combining_text_and_hide_image_placeholders() {
    let mut terminal = crate::ghostty::Terminal::new(40, 3, 1024 * 1024).unwrap();
    terminal.write("old\r\n".repeat(100).as_bytes());
    terminal.write("界 e\u{301} \u{10eeee} tail  ".as_bytes());
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    assert_eq!(pane.recent_text(1), "界 e\u{301}   tail\n");
    let detection = pane.detection_text();
    assert_eq!(detection, "old\nold\n界 e\u{301}   tail\n");
    pane.set_scroll_offset_from_bottom(100);
    assert_eq!(pane.detection_text(), detection);
}

#[test]
fn visible_ansi_preserves_cell_style_sequences() {
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"\x1b[31;1mred\x1b[0m plain");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let ansi = pane.visible_ansi();
    assert!(ansi.contains("red"));
    assert!(ansi.contains("plain"));
    assert!(ansi.contains("\x1b["));
}

#[test]
fn recent_ansi_can_read_styled_scrollback() {
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"\x1b[34mblue\x1b[0m\r\nline2\r\nline3\r\nline4");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let ansi = pane.recent_ansi(4);
    assert!(ansi.contains("blue"));
    assert!(ansi.contains("line4"));
    assert!(ansi.contains("\x1b["));
}

#[test]
fn resize_shrinks_both_axes_with_cursor_at_old_bottom() {
    let mut terminal = crate::ghostty::Terminal::new(8, 4, 10_000).unwrap();
    terminal.write(b"alpha\r\nbeta\r\ngamma\r\ndelta");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    pane.resize(3, 7, 8, 16);

    assert_eq!(pane.visible_text(), "beta\ngamma\ndelta\n");
    assert_eq!(pane.detection_text(), "beta\ngamma\ndelta\n");
    assert_eq!(
        pane.scroll_metrics(),
        Some(ScrollMetrics {
            offset_from_bottom: 0,
            max_offset_from_bottom: 1,
            viewport_rows: 3,
        })
    );
}

#[test]
fn resize_reflow_keeps_scrolled_viewport_and_bottom_detection_sane() {
    let mut terminal = crate::ghostty::Terminal::new(12, 4, 10_000).unwrap();
    write_wrapped_contract_lines(&mut terminal, 40);
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let bottom_snapshot = pane.detection_text();
    assert!(bottom_snapshot.contains("END"));

    let initial = pane.scroll_metrics().expect("initial scroll metrics");
    assert!(initial.max_offset_from_bottom > 0);
    pane.set_scroll_offset_from_bottom(initial.max_offset_from_bottom / 2);
    assert!(!pane.visible_text().trim().is_empty());

    for (rows, cols) in [(4, 10), (4, 7), (6, 18), (3, 9), (5, 12)] {
        let before_resize = pane.scroll_metrics().expect("scroll metrics before resize");
        pane.resize(rows, cols, 0, 0);

        let metrics = pane.scroll_metrics().expect("scroll metrics after resize");
        assert_eq!(metrics.viewport_rows, rows as usize);
        assert_eq!(
            metrics.offset_from_bottom,
            before_resize
                .offset_from_bottom
                .min(metrics.max_offset_from_bottom)
        );
        assert!(
            metrics.offset_from_bottom > 0,
            "resize should preserve a scrolled viewport instead of jumping to bottom"
        );
        assert!(metrics.max_offset_from_bottom > 0);
        let visible = pane.visible_text();
        assert!(
            !visible.trim().is_empty(),
            "visible text should not be empty after resize to {rows}x{cols}; metrics={metrics:?}; detection={:?}; recent={:?}",
            pane.detection_text(),
            pane.recent_text(6)
        );
        assert!(
            pane.detection_text().contains("END"),
            "bottom detection should remain independent from the scrolled viewport after resize"
        );
    }
}

#[test]
fn resize_recovery_does_not_replay_history_when_visible_screen_was_blank() {
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 10_000).unwrap();
    terminal.write(b"old history\r\n\x1b[2J\x1b[H");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert!(pane.visible_text().trim().is_empty());
    assert!(pane.detection_text().trim().is_empty());

    pane.resize(3, 20, 0, 0);

    assert!(pane.visible_text().trim().is_empty());
    assert!(pane.detection_text().trim().is_empty());
    assert!(pane.recent_text(3).trim().is_empty());
}

#[test]
fn resize_recovery_does_not_replay_scrolled_history_over_blank_bottom() {
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 10_000).unwrap();
    write_numbered_lines(&mut terminal, 20);
    terminal.write(b"\x1b[2J\x1b[H");
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    assert!(pane.detection_text().trim().is_empty());
    let metrics = pane.scroll_metrics().expect("scroll metrics");
    pane.set_scroll_offset_from_bottom(metrics.max_offset_from_bottom);
    assert!(!pane.visible_text().trim().is_empty());

    pane.resize(3, 20, 0, 0);

    assert!(pane.detection_text().trim().is_empty());
    assert!(pane.recent_text(3).trim().is_empty());
}

#[test]
fn process_pty_bytes_answers_xtwinops_size_queries() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.resize(24, 80, 9, 18);

    let result = pane.process_pty_bytes(pane_id, b"\x1b[14t\x1b[16t\x1b[18t");

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b[4;432;720t"),
            Bytes::from_static(b"\x1b[6;18;9t"),
            Bytes::from_static(b"\x1b[8;24;80t"),
        ]
    );
}

#[test]
fn xtwinops_size_queries_follow_successful_resize() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.resize(24, 80, 9, 18);
    pane.resize(30, 100, 10, 20);

    let result = pane.process_pty_bytes(pane_id, b"\x1b[14t\x1b[16t\x1b[18t");

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b[4;600;1000t"),
            Bytes::from_static(b"\x1b[6;20;10t"),
            Bytes::from_static(b"\x1b[8;30;100t"),
        ]
    );
}

#[test]
fn xtwinops_size_queries_stay_silent_without_pixel_geometry() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    for (cell_width_px, cell_height_px) in [(0, 0), (0, 18), (9, 0)] {
        pane.resize(24, 80, cell_width_px, cell_height_px);
        let result = pane.process_pty_bytes(pane_id, b"\x1b[14t\x1b[16t\x1b[18t");
        assert!(result.terminal_responses.is_empty());
    }
}

#[test]
fn enabling_in_band_size_reports_after_alt_screen_resize_reports_current_size() {
    let terminal = crate::ghostty::Terminal::new(91, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, b"\x1b[?1049h");
    assert!(pane.resize(24, 92, 9, 18).is_empty());

    let result = pane.process_pty_bytes(pane_id, b"\x1b[?2048h");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b[48;24;92;432;828t")]
    );
}

#[test]
fn resize_returns_in_band_size_report_response() {
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.mode_set(2048, true).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let responses = pane.resize(40, 100, 9, 18);

    assert_eq!(
        responses,
        vec![Bytes::from_static(b"\x1B[48;40;100;720;900t")]
    );
}

#[test]
fn synchronized_output_suppresses_intermediate_render_requests_until_batch_ends() {
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert_eq!(pane_terminal.synchronized_output_state(), (false, 0));
    pane_terminal.process_pty_bytes(pane_id, b"ordinary output");
    assert_eq!(pane_terminal.synchronized_output_state(), (false, 0));

    let begin = pane_terminal.process_pty_bytes(pane_id, b"\x1b[?2026h");
    assert!(!begin.request_render);
    assert_eq!(pane_terminal.synchronized_output_state(), (true, 1));

    let body = pane_terminal.process_pty_bytes(pane_id, b"hello");
    assert!(!body.request_render);
    assert_eq!(pane_terminal.synchronized_output_state(), (true, 1));

    let end = pane_terminal.process_pty_bytes(pane_id, b"\x1b[?2026l");
    assert!(end.request_render);
    assert_eq!(pane_terminal.synchronized_output_state(), (false, 2));
}

#[test]
fn kitty_graphics_write_requests_render_with_settle_backstop() {
    crate::kitty_graphics::set_enabled(true);
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane_terminal.process_pty_bytes(
        pane_id,
        b"\x1b_Ga=T,f=32,t=d,i=7,p=1,s=1,v=1,q=2;/wAA/w==\x1b\\",
    );

    assert!(result.request_render);
    assert_eq!(result.render_delay, Some(KITTY_GRAPHICS_REDRAW_SETTLE));
}

#[test]
fn seeded_history_is_rendered_on_next_draw() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_history_ansi("restored history");

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let row = (0..16).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
    assert_eq!(row, "restored history");
}

#[test]
fn new_pane_terminal_defers_render_snapshot_and_keeps_baseline_colors() {
    let terminal = crate::ghostty::Terminal::new(200, 60, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();

    let mut reference_state = crate::ghostty::RenderState::new().unwrap();
    reference_state
        .update(&crate::ghostty::Terminal::new(1, 1, 0).unwrap())
        .unwrap();
    let reference = reference_state.colors().unwrap();

    let core = pane.core.lock().unwrap();
    assert_eq!(core.render_state.rows().unwrap(), 0);
    assert_eq!(core.render_state.cols().unwrap(), 0);
    assert_eq!(core.theme.initial_foreground, Some(reference.foreground));
    assert_eq!(core.theme.initial_background, Some(reference.background));
}

#[test]
fn pane_created_large_renders_correctly_after_shrinking_before_first_draw() {
    let terminal = crate::ghostty::Terminal::new(200, 60, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_history_ansi("restored\r\n");
    pane.resize(5, 20, 0, 0);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal
            .write(b"\x1b]10;#abcdef\x07\x1b]11;#123456\x07hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), true))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let row = |y| (0..20).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    assert_eq!(row(0).trim_end(), "restored");
    assert_eq!(row(1).trim_end(), "hi");
    assert_eq!(
        buffer[(0, 1)].style().fg,
        Some(Color::Rgb(0xab, 0xcd, 0xef))
    );
    assert_eq!(
        buffer[(0, 1)].style().bg,
        Some(Color::Rgb(0x12, 0x34, 0x56))
    );
    assert_eq!(
        buffer[(19, 4)].style().bg,
        Some(Color::Rgb(0x12, 0x34, 0x56))
    );
    let cursor = pane.cursor_state().unwrap();
    assert_eq!((cursor.x, cursor.y), (2, 1));
}

#[test]
fn render_leaves_unknown_host_default_background_transparent() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_blanks_kitty_unicode_placeholders_when_graphics_enabled() {
    crate::kitty_graphics::set_enabled(true);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal
            .write("before\u{10eeee}\u{0305}\u{0305}after".as_bytes());
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();
    crate::kitty_graphics::set_enabled(false);

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "b");
    assert_eq!(buffer[(6, 0)].symbol(), " ");
    assert_eq!(buffer[(7, 0)].symbol(), "a");
    assert_eq!(pane.visible_text().lines().next(), Some("before after"));
    assert_eq!(pane.recent_text(5), "before after\n");
}

#[test]
fn render_keeps_explicit_cell_foreground_when_host_is_unknown() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[38;2;68;85;102mhi\x1b[0m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_fg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, expected_fg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
}

#[test]
fn render_keeps_explicit_cell_background_when_host_is_unknown() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;2;68;85;102mhi\x1b[0m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_preserves_palette_colors_instead_of_flattening_to_rgb() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(
            b"\x1b[31mR\x1b[0m \x1b[38;5;171mI\x1b[0m \x1b[48;5;4mB\x1b[0m \x1b[38;2;1;2;3mT",
        );
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "R");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Indexed(1)));
    assert_eq!(buffer[(2, 0)].symbol(), "I");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Indexed(171)));
    assert_eq!(buffer[(4, 0)].symbol(), "B");
    assert_eq!(buffer[(4, 0)].style().bg, Some(Color::Indexed(4)));
    assert_eq!(buffer[(6, 0)].symbol(), "T");
    assert_eq!(buffer[(6, 0)].style().fg, Some(Color::Rgb(1, 2, 3)));
}

#[test]
fn render_preserves_palette_background_fill_cells() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;5;4m\x1b[K");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    for x in 0..20 {
        assert_eq!(buffer[(x, 0)].symbol(), " ");
        assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Indexed(4)));
    }
}

#[test]
fn render_preserves_rgb_background_fill_cells() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;2;17;34;51m\x1b[K");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    for x in 0..20 {
        assert_eq!(buffer[(x, 0)].symbol(), " ");
        assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Rgb(17, 34, 51)));
    }
}

#[test]
fn process_pty_bytes_does_not_advertise_unsupported_glyph_protocol() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b_25a1;s\x1b\\");

    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_libghostty_query_responses_without_queuing_input() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b[6n");

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(String::from_utf8_lossy(&result.terminal_responses[0]).contains('R'));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn color_scheme_queries_and_live_updates_follow_terminal_mode() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());
    let query = pane.process_pty_bytes(pane_id, b"\x1b[?996n");
    assert_eq!(
        query.terminal_responses,
        vec![Bytes::from_static(b"\x1b[?997;1n")]
    );

    pane.process_pty_bytes(pane_id, b"\x1b[?2031h");
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());
    assert_eq!(
        pane.apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light)),
        Some(Bytes::from_static(b"\x1b[?997;2n"))
    );

    assert!(pane.apply_host_terminal_appearance(None).is_none());
    let unknown_query = pane.process_pty_bytes(pane_id, b"\x1b[?996n");
    assert!(unknown_query.terminal_responses.is_empty());
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());

    pane.process_pty_bytes(pane_id, b"\x1bc");
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light))
        .is_none());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_xtgettcap_truecolor_query_responses_without_queuing_input() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(
        pane_id,
        b"\x1bP+q5463;524742;73657472676266;73657472676262\x1b\\",
    );

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5463", None),
            expected_xtgettcap_response("524742", Some(b"8")),
            expected_xtgettcap_response("73657472676266", Some(b"\\E[38:2:%p1%d:%p2%d:%p3%dm")),
            expected_xtgettcap_response("73657472676262", Some(b"\\E[48:2:%p1%d:%p2%d:%p3%dm")),
        ]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn mixed_protocol_responses_are_identical_at_every_chunk_boundary() {
    fn run<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> Vec<Bytes> {
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal).unwrap();
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        });
        let mut replies = Vec::new();
        for chunk in chunks {
            replies.extend(
                pane.process_pty_bytes(PaneId::from_raw(1), chunk)
                    .terminal_responses,
            );
            assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
        }
        replies
    }

    let input = b"\x1b[6n\x1b]10;?\x07\x1bP+q524742\x1b\\\x90+q5463;524742\x9c\x1b]10;#aabbcc\x07\x1b]10;?\x1b\\\x1b]110\x07\x1b]10;?\x07\x1b]4;2;#123456\x07\x1b]4;2;?\x1b\\\x1b]12;?\x07\x1b[H\x1b[6n";
    let expected = vec![
        Bytes::from_static(b"\x1b[1;1R"),
        Bytes::from_static(b"\x1b]10;rgb:1111/2222/3333\x1b\\"),
        expected_xtgettcap_response("524742", Some(b"8")),
        expected_xtgettcap_response("5463", None),
        expected_xtgettcap_response("524742", Some(b"8")),
        Bytes::from_static(b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\"),
        Bytes::from_static(b"\x1b]10;rgb:1111/2222/3333\x1b\\"),
        Bytes::from_static(b"\x1b]4;2;rgb:1212/3434/5656\x1b\\"),
        Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x1b\\"),
        Bytes::from_static(b"\x1b[1;1R"),
    ];
    assert_eq!(run([input.as_slice()]), expected, "one chunk");
    assert_eq!(run(input.chunks(1)), expected, "byte by byte");
    for cut in 0..=input.len() {
        assert_eq!(run([&input[..cut], &input[cut..]]), expected, "cut={cut}");
    }
}

#[test]
fn live_effects_discard_restored_callbacks_and_drain_once() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    pane.seed_history_ansi("\x07\x1b]52;c;c3RhbGU=\x07\x1b]7;file:///tmp/restored\x07");
    let id = PaneId::from_raw(1);
    let live = pane.process_pty_bytes(
        id,
        b"\x07\x1b]52;c;bGl2ZQ==\x07\x1b]7;file:///tmp/live\x07\x1b]7;file:///tmp/latest\x07\x07",
    );
    assert_eq!(live.terminal_bells, 2);
    assert_eq!(live.clipboard_writes, vec![b"live".to_vec()]);
    assert_eq!(
        live.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/latest"))
    );
    let drained = pane.process_pty_bytes(id, b"");
    assert_eq!(drained.terminal_bells, 0);
    assert!(drained.clipboard_writes.is_empty());
    assert_eq!(drained.reported_cwd, None);
}

#[test]
fn process_pty_bytes_returns_fragmented_c1_xtgettcap_once_in_order() {
    for query in [
        b"\x90+q5463;524742\x9c".as_slice(),
        b"\x1bP+q5463;524742\x9c".as_slice(),
        b"\x90+q5463;524742\x1b\\".as_slice(),
    ] {
        for fragmented in [false, true] {
            let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
            let pane = GhosttyPaneTerminal::new(terminal).unwrap();
            let pane_id = PaneId::from_raw(1);
            pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
                background: Some(crate::terminal_theme::RgbColor {
                    r: 0,
                    g: 0x2b,
                    b: 0x36,
                }),
                ..Default::default()
            });
            let mut replies = pane
                .process_pty_bytes(pane_id, b"\x1b]11;?\x07")
                .terminal_responses;
            for chunk in query.chunks(if fragmented { 1 } else { query.len() }) {
                replies.extend(pane.process_pty_bytes(pane_id, chunk).terminal_responses);
            }
            replies.extend(
                pane.process_pty_bytes(pane_id, b"\x1b]11;?\x1b\\\x1bP+q5375\x1b\\")
                    .terminal_responses,
            );
            assert_eq!(
                replies,
                vec![
                    Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\"),
                    expected_xtgettcap_response("5463", None),
                    expected_xtgettcap_response("524742", Some(b"8")),
                    Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\"),
                    expected_xtgettcap_response("5375", None),
                ],
                "query={query:?}, fragmented={fragmented}"
            );
            assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn process_pty_bytes_returns_split_xtgettcap_query_response() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1bP+q4");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"d73");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    // libghostty unhooks DCS on ESC, before the final ST backslash.
    // Splitting ST must not lose the reply or emit it again on completion.
    let result = pane.process_pty_bytes(pane_id, b"\x1b");

    assert_eq!(
        result.terminal_responses,
        vec![expected_xtgettcap_response(
            "4D73",
            Some(b"\\E]52;%p1%s;%p2%s\\007")
        )]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"\\");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_orders_device_attribute_reply_before_following_xtgettcap_reply() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b[c\x1bP+q5463\x1b\\");

    assert_eq!(result.terminal_responses.len(), 2);
    assert!(String::from_utf8_lossy(&result.terminal_responses[0]).contains('c'));
    assert_eq!(
        result.terminal_responses[1],
        expected_xtgettcap_response("5463", None)
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_orders_xtgettcap_reply_before_following_device_attribute_reply() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1bP+q5463\x1b\\\x1b[c");

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0],
        expected_xtgettcap_response("5463", None)
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[1]).contains('c'));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_orders_xtgettcap_reply_before_following_default_color_reply() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1bP+q5463\x1b\\\x1b]11;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5463", None),
            Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\"),
        ]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn host_theme_update_preserves_child_default_color_override() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;#112233\x07");
    assert!(result.terminal_responses.is_empty());

    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;?\x07");
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:1111/2222/3333\x07")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn child_default_color_reset_restores_cached_host_color() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"\x1b]11;#112233\x07");
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        ..Default::default()
    });
    pane.process_pty_bytes(pane_id, b"\x1b]111\x07");
    assert!(!pane.has_transient_default_color_override());

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;?\x07");
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:aaaa/bbbb/cccc\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_recovers_xtgettcap_after_osc_bel_terminator() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b]0;title\x07\x1bP+q5463\x1b\\");

    assert_eq!(
        result.terminal_responses,
        vec![expected_xtgettcap_response("5463", None)]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_orders_default_color_reset_reply_before_xtgettcap() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;#112233\x07\x1b]111\x07\x1b]11;?\x1b");
    assert!(result.terminal_responses.is_empty());
    let result = pane.process_pty_bytes(pane_id, b"\\\x1bP+q436f\x1b\\");

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\"),
            // Co is provided by the native map, beyond our old eight keys.
            expected_xtgettcap_response("436F", Some(b"256")),
        ]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_ignores_unknown_and_unsupported_xtgettcap_queries() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1bP+q6E6F7065;4D7\x1b\\");

    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_underline_color_xtgettcap_query_responses() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1bP+q5375;536D756C78;536574756C63\x1b\\");

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5375", None),
            expected_xtgettcap_response("536D756C78", Some(b"\\E[4:%p1%dm")),
            expected_xtgettcap_response(
                "536574756C63",
                Some(b"\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m")
            ),
        ]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn render_preserves_underline_color() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[4m\x1b[58:2::17:34:51mU");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let style = terminal.backend().buffer()[(0, 0)].style();
    assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(style.underline_color, Some(Color::Rgb(17, 34, 51)));
}

#[test]
fn full_frame_preserves_curly_underline_style() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[4:3mU");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let frame = crate::protocol::FrameData::from_ratatui_buffer(terminal.backend().buffer(), None);
    assert_eq!(frame.cells[0].symbol, "U");
    assert_eq!(
        crate::protocol::underline_style_from_modifier(frame.cells[0].modifier),
        3
    );
}

#[test]
fn process_pty_bytes_orders_default_color_reply_before_following_device_attribute_reply() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;?\x07\x1b[c");

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0],
        Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\")
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[1]).contains('c'));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_host_palette_color_without_queuing_input() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            0,
            crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
        ),
    );

    let result = pane.process_pty_bytes(pane_id, b"\x1b]4;0;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;0;rgb:1111/2222/3333\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn opentui_256_palette_query_burst_uses_host_snapshot() {
    use std::fmt::Write as _;

    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    let mut theme = crate::terminal_theme::TerminalTheme::default();
    let mut queries = String::new();
    for index in 0..=u8::MAX {
        theme = theme.with_palette_color(
            index,
            crate::terminal_theme::RgbColor {
                r: index,
                g: 0x22,
                b: 0x33,
            },
        );
        let _ = write!(queries, "\x1b]4;{index};?\x07");
    }
    pane.apply_host_terminal_theme(theme);

    let result = pane.process_pty_bytes(pane_id, queries.as_bytes());

    assert_eq!(result.terminal_responses.len(), 256);
    assert_eq!(
        result.terminal_responses[0],
        Bytes::from_static(b"\x1b]4;0;rgb:0000/2222/3333\x1b\\")
    );
    assert_eq!(
        result.terminal_responses[255],
        Bytes::from_static(b"\x1b]4;255;rgb:ffff/2222/3333\x1b\\")
    );
}

#[test]
fn child_palette_override_survives_host_refresh_until_reset() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            7,
            crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
        ),
    );
    pane.process_pty_bytes(pane_id, b"\x1b]4;7;rgb:aa/bb/cc\x1b\\");

    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            7,
            crate::terminal_theme::RgbColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
            },
        ),
    );
    let overridden = pane.process_pty_bytes(pane_id, b"\x1b]4;7;?\x1b\\");
    assert_eq!(
        overridden.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;7;rgb:aaaa/bbbb/cccc\x1b\\")]
    );

    pane.process_pty_bytes(pane_id, b"\x1b]104;7\x1b\\");
    let reset = pane.process_pty_bytes(pane_id, b"\x1b]4;7;?\x1b\\");
    assert_eq!(
        reset.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;7;rgb:4444/5555/6666\x1b\\")]
    );
}

#[test]
fn process_pty_bytes_returns_split_palette_color_query_response() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    let color = current_palette_color(&pane, 255);

    let result = pane.process_pty_bytes(pane_id, b"\x1b]4;25");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"5;?\x1b");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"\\");

    assert_eq!(
        result.terminal_responses,
        vec![expected_osc_rgb_response("4;255", color)]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_ignores_malformed_and_preserves_multi_palette_queries() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b]4;;?\x07\x1b]4;-1;?\x07\x1b]4;256;?\x07\x1b]4;0;?;1;?\x07\x1b]4;0;rgb:1111/2222/3333\x07");

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(result.terminal_responses[0].starts_with(b"\x1b]4;0;rgb:"));
    assert_eq!(
        result.terminal_responses[0]
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_orders_palette_reply_before_following_terminal_replies() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    let color = current_palette_color(&pane, 0);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]4;0;?\x07\x1b]11;?\x07\x1b[c");

    assert_eq!(result.terminal_responses.len(), 3);
    assert_eq!(
        result.terminal_responses[0],
        expected_osc_rgb_response("4;0", color)
    );
    assert_eq!(
        result.terminal_responses[1],
        Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\")
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[2]).contains('c'));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_default_color_query_responses_without_queuing_input() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_preserves_untracked_multi_color_query_responses() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let palette = pane.process_pty_bytes(pane_id, b"\x1b]4;0;?;1;?\x1b\\");
    let palette_response = palette.terminal_responses.concat();
    assert!(palette_response.starts_with(b"\x1b]4;0;rgb:"));
    assert_eq!(
        palette_response
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );

    let defaults = pane.process_pty_bytes(pane_id, b"\x1b]10;?;?;?\x1b\\");
    let default_response = defaults.terminal_responses.concat();
    assert!(
        default_response.starts_with(b"\x1b]10;rgb:"),
        "unexpected default-color report: {:?}",
        String::from_utf8_lossy(&default_response)
    );
    assert_eq!(
        default_response
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        3
    );
    let core = pane.core.lock().unwrap();
    assert!(!core.theme.child_foreground_changed);
    assert!(!core.theme.child_background_changed);
    drop(core);
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_preserves_earlier_aggregate_palette_reply() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, b"\x1b]4;0;?;1;?\x1b\\\x1b]4;0;?\x1b\\");

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0]
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );
    assert!(result.terminal_responses[1].starts_with(b"\x1b]4;0;rgb:"));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_preserves_libghostty_reply_for_child_color_override() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"\x1b]10;rgb:11/22/33\x07");
    let result = pane.process_pty_bytes(pane_id, b"\x1b]10;?\x1b\\");

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(result.terminal_responses[0].starts_with(b"\x1b]10;rgb:1111/2222/3333"));
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_tracks_later_multi_value_color_set() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, b"\x1b]10;?;rgb:44/55/66\x1b\\");

    let core = pane.core.lock().unwrap();
    assert!(!core.theme.child_foreground_changed);
    assert!(core.theme.child_background_changed);
}

#[test]
fn process_pty_bytes_returns_cursor_color_query_response_from_foreground_fallback() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]12;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_cursor_color_query_response_from_child_foreground() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    pane.process_pty_bytes(pane_id, b"\x1b]10;rgb:11/22/33\x07");
    let result = pane.process_pty_bytes(pane_id, b"\x1b]12;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_explicit_cursor_color_query_response() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    pane.process_pty_bytes(pane_id, b"\x1b]12;rgb:11/22/33\x07");
    let result = pane.process_pty_bytes(pane_id, b"\x1b]12;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_default_color_query_responses_in_order() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b]10;rgb:6565/7b7b/8383\x1b\\"),
            Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\"),
            Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x1b\\"),
        ]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_split_default_color_query_response() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b";?\x1b");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"\\");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_returns_split_cursor_color_query_response() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        background: None,
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]12");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b";?\x1b");
    assert!(result.terminal_responses.is_empty());
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
    let result = pane.process_pty_bytes(pane_id, b"\\");

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:fdfd/f6f6/e3e3\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn process_pty_bytes_tracks_default_color_set_and_reset_before_replying() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, b"\x1b]11;rgb:11/22/33\x07\x1b]11;?\x07");
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:1111/2222/3333\x07")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());

    let result = pane.process_pty_bytes(pane_id, b"\x1b]111\x07\x1b]11;?\x07");
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\")]
    );
    assert!(pane.pending_pty_responses.lock().unwrap().is_empty());
}

#[test]
fn render_leaves_host_default_background_transparent() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let host_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_keeps_explicit_default_foreground_when_it_differs_from_host() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let host_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b]10;rgb:44/55/66\x1b\\hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_fg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, expected_fg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, expected_fg);
}

#[test]
fn render_keeps_explicit_default_background_when_it_differs_from_host() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let host_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b]11;rgb:44/55/66\x1b\\hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().bg, expected_bg);
}

#[test]
fn render_inverse_text_swaps_fg_and_resolved_bg_when_bg_is_transparent() {
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal).unwrap();
    let host_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        // SGR 7 enables inverse/reverse video
        core.terminal.write(b"\x1b[7mhi\x1b[27m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let cell = &buffer[(0, 0)];
    assert_eq!(cell.symbol(), "h");
    // After inverse: fg should be the resolved bg, bg should be the original fg.
    // fg must NOT be Color::Reset (which would be the same hue as bg).
    assert_eq!(cell.style().fg, Some(Color::Rgb(0x11, 0x22, 0x33)));
    assert_eq!(cell.style().bg, Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
}
