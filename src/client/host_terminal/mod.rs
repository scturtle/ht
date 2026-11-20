//! The client's outer terminal: lifecycle, capabilities, input modes and geometry.
//! Pane emulation and shared terminal-theme data do not belong in this module.

mod geometry;
mod modes;
mod probe;
mod setup;
mod theme;

#[cfg(test)]
mod tests;

pub(super) use geometry::{
    current_terminal_geometry, ioctl_terminal_geometry, reported_cell_size_from_events,
    resize_poll_loop, store_reported_cell_size, HostGeometry, HostPixels,
};
pub(super) use modes::{
    clear_host_mouse_reporting, effective_mouse_capture, effective_sgr_pixel_mouse,
    set_host_kitty_keyboard_report_all, set_mouse_capture,
};
pub(super) use setup::{setup_terminal, TerminalGuard};
pub(super) use theme::query_host_terminal_theme;
