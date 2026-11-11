//! Original zoom transition, owned by each client's presentation, not Core BSP.
use super::model::{invalid, Model};
use crate::{api::schema as api, app::actions::PaneZoomOutcome, layout::PaneId};
use std::io;

impl Model {
    pub(super) fn active_zoomed(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| self.zoomed_tabs.contains(tab))
    }

    pub(super) fn zoom_pane(
        &mut self,
        id: PaneId,
        mode: api::PaneZoomMode,
    ) -> io::Result<PaneZoomOutcome> {
        let (workspace, tab) = self
            .context(id)
            .ok_or_else(|| invalid("Core pane not found"))?;
        let layout = self
            .layouts
            .get(&tab.id)
            .ok_or_else(|| invalid("Core tab not found"))?;
        let focus_changed = self.active_workspace != workspace.id
            || self.active_tab() != Some(tab.id.as_str())
            || layout.focused() != id;
        let tab = tab.id.clone();
        let outcome = crate::app::actions::pane_zoom(
            self.zoomed_tabs.contains(&tab),
            layout.pane_count(),
            mode,
            focus_changed,
        );
        // Native behavior focuses even when zoom itself is a single-pane/idempotent no-op.
        if focus_changed {
            self.focus(id)?;
        }
        if outcome.changed {
            if outcome.zoomed {
                self.zoomed_tabs.insert(tab);
            } else {
                self.zoomed_tabs.remove(&tab);
            }
            self.revision += 1;
        }
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
