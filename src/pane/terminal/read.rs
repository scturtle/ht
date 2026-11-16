//! Locked text capture, selection and copy-mode queries.
//! Pure search/motion algorithms remain in text.rs; bounded scans stay bounded here.

use super::text::{terminal_cell_text, RetainedTextBuffer};
use super::{
    GhosttyPaneCore, GhosttyPaneTerminal, PaneTerminal, TerminalReadSnapshot,
    TerminalSearchDirection, TerminalSearchWindow, TerminalTextPoint, TerminalWordMotion,
};

pub(super) const DEFAULT_DETECTION_ROWS: usize = 24;

impl PaneTerminal {
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
}

impl GhosttyPaneTerminal {
    pub fn extract_selection(&self, selection: &crate::selection::Selection) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_extract_selection(&core.terminal, selection).ok())
    }
}

pub(super) fn ghostty_visible_text(
    core: &mut GhosttyPaneCore,
) -> Result<String, crate::ghostty::Error> {
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

pub(super) fn ghostty_detection_text(
    terminal: &crate::ghostty::Terminal,
) -> Result<String, crate::ghostty::Error> {
    ghostty_detection_text_for_cache(terminal).map(|(text, _)| text)
}

pub(super) fn ghostty_detection_text_for_cache(
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

pub(super) fn finish_detection_text(text: String) -> (String, bool) {
    let reusable = !text.trim().is_empty();
    (text, reusable)
}

pub(super) fn ghostty_recent_ansi(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<String, crate::ghostty::Error> {
    ghostty_recent_ansi_snapshot(terminal, lines, unwrap).map(|snapshot| snapshot.text)
}

pub(super) fn ghostty_recent_ansi_snapshot(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_ansi_for_terminal(terminal, lines, unwrap)?;
    Ok(finish_recent_snapshot(terminal, text, lines))
}

pub(super) fn finish_recent_snapshot(
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

pub(super) fn ghostty_recent_text_for_terminal(
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

pub(super) fn ghostty_recent_read_range(
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

pub(super) fn ghostty_cell_symbol(
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

fn trim_trailing_blank_rows(rows: &mut Vec<String>) {
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
