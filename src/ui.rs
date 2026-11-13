//! Pure client-side drawing and geometry.
mod colors;
mod palette;
pub(crate) use colors::palette_from_config;
pub(crate) use palette::Palette;
pub(crate) mod dialog;
mod panes;
mod scrollbar;
mod status;
mod text;
pub(crate) mod widgets;
pub(crate) use panes::{
    apply_pane_chrome, pane_inner_rect, render_selection_highlight, zoomed_pane_info,
};
pub(crate) use scrollbar::{
    render_scrollbar_buffer, scrollbar_offset_from_drag_row, scrollbar_offset_from_row,
    scrollbar_thumb, scrollbar_thumb_grab_offset,
};
pub(crate) use status::{render_config_diagnostic_buffer, render_copy_feedback_buffer};
pub(crate) use text::truncate_end;
pub(crate) use widgets::panel_contrast_fg;

use ratatui::layout::Rect;
use status::copy_feedback_rect;
pub(crate) fn copy_feedback_offset_for_toast(
    area: Rect,
    feedback: &str,
    base_offset: u16,
    position: crate::config::ToastClipboardPosition,
    toast_rect: Rect,
) -> u16 {
    let feedback_rect = copy_feedback_rect(area, feedback, base_offset, position);
    if rectangles_overlap(feedback_rect, toast_rect) {
        base_offset.saturating_add(toast_rect.height)
    } else {
        base_offset
    }
}

fn rectangles_overlap(left: Rect, right: Rect) -> bool {
    left.x < right.right()
        && right.x < left.right()
        && left.y < right.bottom()
        && right.y < left.bottom()
}
