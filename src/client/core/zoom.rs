//! Original zoom transition, owned by each client's presentation, not Core BSP.
use super::model::{invalid, Model};
use crate::{actions, layout::PaneId};
use std::io;

/// Pure transition extracted from the original AppState::apply_pane_zoom.
/// Focus is resolved by the client before single-pane/idempotence checks.
pub(super) struct PaneZoomOutcome {
    pub changed: bool,
    pub focus_changed: bool,
    pub reason: Option<crate::actions::PaneZoomReason>,
    pub zoomed: bool,
}

fn pane_zoom(
    zoomed: bool,
    pane_count: usize,
    command: crate::actions::PaneZoomMode,
    focus_changed: bool,
) -> PaneZoomOutcome {
    use crate::actions::{PaneZoomMode as Command, PaneZoomReason as Reason};
    if pane_count <= 1 {
        return PaneZoomOutcome {
            changed: false,
            focus_changed,
            reason: Some(Reason::SinglePane),
            zoomed,
        };
    }
    let desired = match command {
        Command::Toggle => !zoomed,
        Command::On => true,
        Command::Off => false,
    };
    let reason = match (command, zoomed) {
        (Command::On, true) => Some(Reason::AlreadyZoomed),
        (Command::Off, false) => Some(Reason::AlreadyUnzoomed),
        _ => None,
    };
    PaneZoomOutcome {
        changed: reason.is_none(),
        focus_changed,
        zoomed: if reason.is_some() { zoomed } else { desired },
        reason,
    }
}

impl Model {
    pub(super) fn active_zoomed(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| self.zoomed_tabs.contains(tab))
    }

    pub(super) fn zoom_pane(
        &mut self,
        id: PaneId,
        mode: actions::PaneZoomMode,
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
        let outcome = pane_zoom(
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
