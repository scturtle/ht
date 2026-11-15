use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tracing::debug;

use super::ClientLoopEvent;

const DEFAULT_CELL_SIZE: (u32, u32) = (8, 16);

pub(super) fn ioctl_terminal_geometry() -> Option<(u16, u16, u32, u32)> {
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
pub(super) fn current_terminal_geometry(
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
pub(super) fn resize_poll_loop(
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

pub(super) fn store_reported_cell_size(
    reported_cell_size: &AtomicU64,
    width_px: u32,
    height_px: u32,
) {
    let packed = pack_cell_size(width_px, height_px);
    if reported_cell_size.swap(packed, Ordering::AcqRel) != packed {
        debug!(width_px, height_px, "host terminal reported cell size");
    }
}

pub(super) fn reported_cell_size_from_events(
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
