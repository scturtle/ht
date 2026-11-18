//! Ordered App panes and strictly client-local focus for one tab.
use super::model::invalid;
use crate::layout::PaneId;
use std::io;

pub(super) struct TabLayout {
    ids: Vec<PaneId>,
    focus: PaneId,
    previous: Option<PaneId>,
}

impl TabLayout {
    pub fn new(ids: Vec<PaneId>, previous_focus: Option<PaneId>) -> io::Result<Self> {
        let focus = previous_focus
            .filter(|id| ids.contains(id))
            .or_else(|| ids.first().copied())
            .ok_or_else(|| invalid("tab has no focus"))?;
        Ok(Self {
            ids,
            focus,
            previous: None,
        })
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn pane_ids(&self) -> &[PaneId] {
        &self.ids
    }

    pub fn focus_pane(&mut self, id: PaneId) {
        if id != self.focus && self.ids.contains(&id) {
            self.previous = Some(self.focus);
            self.focus = id;
        }
    }

    /// The caller removes the whole tab instead of closing its last pane.
    pub fn remove(&mut self, id: PaneId) {
        if self.ids.len() <= 1 {
            return;
        }
        let Some(index) = self.ids.iter().position(|pane| *pane == id) else {
            return;
        };
        self.ids.remove(index);
        if self.focus == id {
            self.focus = self
                .previous
                .take()
                .unwrap_or_else(|| self.ids[index.min(self.ids.len() - 1)]);
        } else if self.previous == Some(id) {
            self.previous = None;
        }
    }
}
