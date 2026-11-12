//! Pure client-side drawing and geometry.
mod panes;
mod release_notes;
mod scrollbar;
mod sidebar;
mod status;
mod text;
mod widgets;
pub(crate) use panes::{
    apply_pane_chrome, pane_inner_rect, render_selection_highlight, zoomed_pane_info,
};
pub(crate) use release_notes::{
    release_notes_close_button_rect, release_notes_display_lines, release_notes_scroll_metrics,
    RELEASE_NOTES_MODAL_SIZE,
};
pub(crate) use scrollbar::{
    release_notes_scrollbar_rect, render_scrollbar_buffer, scrollbar_offset_from_drag_row,
    scrollbar_offset_from_row, scrollbar_thumb, scrollbar_thumb_grab_offset,
};
pub(crate) use sidebar::{
    expanded_sidebar_sections, resolved_token_spans, sidebar_agent_rows,
    sidebar_section_divider_rect, sidebar_space_rows, AgentTokenContext, ResolvedToken,
    ResolvedTokenKind, SpaceTokenContext,
};
pub(crate) use status::{render_config_diagnostic_buffer, render_copy_feedback_buffer};
pub(crate) use text::truncate_end;
pub(crate) use widgets::{centered_popup_rect, modal_stack_areas};

use ratatui::layout::Rect;
use status::copy_feedback_rect;
pub(crate) fn copy_feedback_offset_for_toast(
    area: Rect,
    feedback: &crate::app::state::CopyFeedback,
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
