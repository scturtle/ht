//! Host grid/pixel sampling, mouse coordinate mapping and resize watching.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tracing::debug;

use crate::client::events::ClientLoopEvent;
use crate::input::mouse::Position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client) struct HostGeometry {
    pub(in crate::client) cols: u16,
    pub(in crate::client) rows: u16,
    pub(in crate::client) width_px: u32,
    pub(in crate::client) height_px: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client) struct HostPixels {
    pub(in crate::client) x: u32,
    pub(in crate::client) y: u32,
    pub(in crate::client) geometry: HostGeometry,
}

impl HostGeometry {
    pub(in crate::client) fn new(
        cols: u16,
        rows: u16,
        width_px: u32,
        height_px: u32,
    ) -> Option<Self> {
        (cols > 0 && rows > 0 && width_px > 0 && height_px > 0).then_some(Self {
            cols,
            rows,
            width_px,
            height_px,
        })
    }

    pub(in crate::client) fn current() -> Option<Self> {
        let size = crossterm::terminal::window_size().ok()?;
        Self::new(
            size.columns,
            size.rows,
            u32::from(size.width),
            u32::from(size.height),
        )
    }

    pub(in crate::client) fn cell(self, x: u32, y: u32) -> Option<(u16, u16)> {
        let x = x.checked_sub(1)?;
        let y = y.checked_sub(1)?;
        if x >= self.width_px || y >= self.height_px {
            return None;
        }
        let width_px = grid_extent(self.cols, self.width_px)?;
        let height_px = grid_extent(self.rows, self.height_px)?;
        Some((
            grid_cell(x.min(width_px - 1), self.cols, width_px)?,
            grid_cell(y.min(height_px - 1), self.rows, height_px)?,
        ))
    }
}

impl HostPixels {
    pub(in crate::client) fn pane_position(
        self,
        inner: ratatui::layout::Rect,
        child_width_px: u32,
        child_height_px: u32,
    ) -> Option<Position> {
        let (host_column, host_row) = self.geometry.cell(self.x, self.y)?;
        let end_column = inner.x.checked_add(inner.width)?;
        let end_row = inner.y.checked_add(inner.height)?;
        if host_column < inner.x
            || host_column >= end_column
            || host_row < inner.y
            || host_row >= end_row
        {
            return None;
        }
        Some(Position::Pixels {
            x: map_axis_within_cell(
                self.x,
                host_column,
                inner.x,
                inner.width,
                self.geometry.cols,
                self.geometry.width_px,
                child_width_px,
            )?,
            y: map_axis_within_cell(
                self.y,
                host_row,
                inner.y,
                inner.height,
                self.geometry.rows,
                self.geometry.height_px,
                child_height_px,
            )?,
        })
    }
}

fn map_axis_within_cell(
    pixel: u32,
    host_cell: u16,
    pane_start: u16,
    pane_cells: u16,
    host_cells: u16,
    host_extent: u32,
    child_extent: u32,
) -> Option<u32> {
    let local_cell = host_cell.checked_sub(pane_start)?;
    if local_cell >= pane_cells {
        return None;
    }
    let host_grid_extent = grid_extent(host_cells, host_extent)?;
    let source_start = boundary(host_cell, host_cells, host_grid_extent)?;
    let source_end = boundary(host_cell.checked_add(1)?, host_cells, host_grid_extent)?;
    let target_start = boundary(local_cell, pane_cells, child_extent)?;
    let target_end = boundary(local_cell.checked_add(1)?, pane_cells, child_extent)?;
    let source_width = source_end.checked_sub(source_start)?;
    let target_width = target_end.checked_sub(target_start)?;
    let offset = pixel.checked_sub(1)?.checked_sub(source_start)?;
    if source_width == 0 || target_width == 0 || offset >= source_width {
        return None;
    }
    target_start
        .checked_add(scale(offset, source_width, target_width))?
        .checked_add(1)
}

fn boundary(index: u16, count: u16, extent: u32) -> Option<u32> {
    (count > 0 && index <= count && extent > 0)
        .then(|| (u64::from(index) * u64::from(extent) / u64::from(count)) as u32)
}

fn grid_extent(count: u16, extent: u32) -> Option<u32> {
    let count = u32::from(count);
    (count > 0 && extent > 0).then(|| (extent / count).max(1) * count)
}

fn grid_cell(pixel: u32, count: u16, extent: u32) -> Option<u16> {
    if count == 0 || extent == 0 || pixel >= extent {
        return None;
    }
    let cell = ((u64::from(pixel) + 1) * u64::from(count) - 1) / u64::from(extent);
    u16::try_from(cell).ok().filter(|cell| *cell < count)
}

fn scale(pixel: u32, source: u32, target: u32) -> u32 {
    ((u64::from(pixel) * u64::from(target)) / u64::from(source))
        .min(u64::from(target.saturating_sub(1))) as u32
}

const DEFAULT_CELL_SIZE: (u32, u32) = (8, 16);

pub(in crate::client) fn ioctl_terminal_geometry() -> Option<(u16, u16, u32, u32)> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    // Pixel extents may include padding, so use the integer average cell pitch.
    Some((
        size.columns,
        size.rows,
        (u32::from(size.width) / u32::from(size.columns)).max(1),
        (u32::from(size.height) / u32::from(size.rows)).max(1),
    ))
}

pub(super) fn cell_size_fallback(reported: u64, last: Option<(u32, u32)>) -> (u32, u32) {
    let width_px = (reported >> 32) as u32;
    let height_px = reported as u32;
    (width_px > 0 && height_px > 0)
        .then_some((width_px, height_px))
        .or(last.filter(|(width, height)| *width > 0 && *height > 0))
        .unwrap_or(DEFAULT_CELL_SIZE)
}

pub(super) fn pack_cell_size(width_px: u32, height_px: u32) -> u64 {
    (u64::from(width_px) << 32) | u64::from(height_px)
}

type TerminalGeometry = (u16, u16, u32, u32, bool);

pub(super) fn current_terminal_geometry_with(
    reported_cell_size: u64,
    last_cell_size: Option<(u32, u32)>,
    exact_geometry: Option<(u16, u16, u32, u32)>,
    terminal_grid_size: impl FnOnce() -> io::Result<(u16, u16)>,
) -> io::Result<TerminalGeometry> {
    if let Some((cols, rows, cell_width_px, cell_height_px)) = exact_geometry {
        return Ok((cols, rows, cell_width_px, cell_height_px, true));
    }
    let (cols, rows) = terminal_grid_size()?;
    let (cell_width_px, cell_height_px) = cell_size_fallback(reported_cell_size, last_cell_size);
    Ok((cols, rows, cell_width_px, cell_height_px, false))
}

/// Pixel geometry is exact only when one ioctl supplies the complete snapshot.
pub(in crate::client) fn current_terminal_geometry(
    reported_cell_size: u64,
    last_cell_size: Option<(u32, u32)>,
) -> io::Result<TerminalGeometry> {
    current_terminal_geometry_with(
        reported_cell_size,
        last_cell_size,
        ioctl_terminal_geometry(),
        crate::platform::terminal_grid_size,
    )
}

/// Watches the terminal size and sends resize events when it changes.
///
/// The baseline cell size must match what the handshake sent to the server:
/// reading a fresh one here would race the host cell size reply and could
/// swallow the first change.
pub(in crate::client) fn resize_poll_loop(
    resize_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    initial_size: TerminalGeometry,
    reported_cell_size: &AtomicU64,
    should_quit: &AtomicBool,
) {
    crate::platform::watch_terminal_resize_signal();
    resize_poll_loop_with(
        resize_tx,
        initial_size,
        |last_cell_size| {
            current_terminal_geometry(
                reported_cell_size.load(Ordering::Acquire),
                Some(last_cell_size),
            )
        },
        crate::platform::take_terminal_resize_signal,
        should_quit,
    );
}

/// The resize watcher loop, with its geometry and resize-signal sources
/// injected so the transient/recovery behavior is testable without a terminal.
pub(super) fn resize_poll_loop_with(
    resize_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    initial_size: TerminalGeometry,
    mut read_geometry: impl FnMut((u32, u32)) -> io::Result<TerminalGeometry>,
    mut take_signal: impl FnMut() -> bool,
    should_quit: &AtomicBool,
) {
    let mut last_size = initial_size;
    // Retained across a transient sample so a resize that lands back on the
    // same size still re-reports (host modes may have been reset).
    let mut signalled = false;
    while !should_quit.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(100));
        signalled |= take_signal();
        let new_size = match read_geometry((last_size.2, last_size.3)) {
            Ok(size) => size,
            // A window drag can momentarily report a zero-sized grid. Keep
            // the last known geometry and the pending signal, then resample
            // instead of treating the terminal as gone.
            Err(err) if crate::platform::is_transient_terminal_grid_error(&err) => continue,
            Err(err) => {
                let _ = resize_tx.blocking_send(ClientLoopEvent::TerminalUnavailable(err));
                break;
            }
        };
        if signalled || new_size != last_size {
            last_size = new_size;
            let (cols, rows, cell_width_px, cell_height_px, exact) = new_size;
            if resize_tx
                .blocking_send(ClientLoopEvent::Resize(
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                    exact,
                ))
                .is_err()
            {
                break;
            }
        }
        signalled = false;
    }
}

pub(in crate::client) fn store_reported_cell_size(
    reported_cell_size: &AtomicU64,
    width_px: u32,
    height_px: u32,
) {
    let packed = pack_cell_size(width_px, height_px);
    if reported_cell_size.swap(packed, Ordering::AcqRel) != packed {
        debug!(width_px, height_px, "host terminal reported cell size");
    }
}

pub(in crate::client) fn reported_cell_size_from_events(
    events: &[crate::raw_input::RawInputEvent],
) -> Option<(u32, u32)> {
    events.iter().rev().find_map(|event| match event {
        crate::raw_input::RawInputEvent::HostCellSizeReport {
            width_px,
            height_px,
        } => Some((*width_px, *height_px)),
        _ => None,
    })
}
