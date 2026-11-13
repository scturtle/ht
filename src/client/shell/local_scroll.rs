use super::*;

/// A viewport the client scrolls locally, without asking Core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalScrollTarget {
    Help,
    Navigator,
}

/// The scrollable track this frame painted for `target`, if it has one.
fn local_scrollbar(
    hits: &ShellHitMap,
    target: LocalScrollTarget,
) -> Option<(Rect, crate::pane::ScrollMetrics)> {
    let (track, metrics) = match target {
        LocalScrollTarget::Help => (hits.help_scrollbar, hits.help_scroll_metrics),
        LocalScrollTarget::Navigator => (hits.navigator_scrollbar, hits.navigator_scroll_metrics),
    };
    let metrics = metrics?;
    (!track.is_empty() && metrics.max_offset_from_bottom > 0).then_some((track, metrics))
}

impl ClientShellState {
    /// Moves the viewport that owns `target` to `offset_from_bottom` and reports
    /// whether its first visible row changed.
    ///
    /// Navigator always reports a change: its selection follows the viewport so
    /// rendering does not snap back to a row outside it.
    pub(super) fn apply_local_scroll(
        &mut self,
        target: LocalScrollTarget,
        metrics: crate::pane::ScrollMetrics,
        offset_from_bottom: usize,
    ) -> bool {
        // Local viewports scroll by their first visible row; the shared math
        // reports the distance from the bottom.
        let start = metrics
            .max_offset_from_bottom
            .saturating_sub(offset_from_bottom);
        match target {
            LocalScrollTarget::Help => {
                let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() else {
                    return false;
                };
                set_local_scroll(&mut help.scroll, start)
            }
            LocalScrollTarget::Navigator => {
                self.scroll_navigator_to(start, metrics.viewport_rows);
                true
            }
        }
    }

    /// Left press on a local scrollbar: grab the thumb or seek the track.
    pub(super) fn press_local_scrollbar(
        &mut self,
        target: LocalScrollTarget,
        row: u16,
        outcome: &mut ClientShellInput,
    ) {
        let Some((track, metrics)) = local_scrollbar(&self.hits, target) else {
            return;
        };
        if let Some(row_offset) = crate::ui::scrollbar_thumb_grab_offset(metrics, track, row) {
            self.chrome_drag = Some(ClientChromeDrag::LocalScrollbar {
                target,
                grab_row_offset: row_offset,
            });
            return;
        }
        let offset = crate::ui::scrollbar_offset_from_row(metrics, track, row);
        let moved = self.apply_local_scroll(target, metrics, offset);
        // A Help track press repaints even when its wrapped offset is unchanged.
        outcome.repaint |= moved || target == LocalScrollTarget::Help;
    }

    /// Left drag on a local scrollbar, keeping the offset grabbed at press time.
    pub(super) fn drag_local_scrollbar(
        &mut self,
        target: LocalScrollTarget,
        row: u16,
        row_offset: u16,
        outcome: &mut ClientShellInput,
    ) {
        let Some((track, metrics)) = local_scrollbar(&self.hits, target) else {
            return;
        };
        let offset = crate::ui::scrollbar_offset_from_drag_row(metrics, track, row, row_offset);
        outcome.repaint |= self.apply_local_scroll(target, metrics, offset);
    }
}

fn set_local_scroll(scroll: &mut usize, start: usize) -> bool {
    let moved = *scroll != start;
    *scroll = start;
    moved
}
