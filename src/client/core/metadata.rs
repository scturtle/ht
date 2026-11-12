//! Shared terminal metadata through existing Core APIs, not TUI-owned storage.
use super::{model::invalid, session::Session};
use crate::actions;
use serde_json::{json, Value};
use std::io;

const LABEL_METHOD: &str = "terminal.label";
const DISPLAY_NAME: &str = "herdr.display_name";

pub(super) fn supports_rename(hello: &Value) -> bool {
    hello["methods"]
        .as_array()
        .is_some_and(|methods| methods.iter().any(|m| m == LABEL_METHOD))
}

pub(super) fn pane_label(facts: &Value) -> Option<String> {
    crate::app::manual_pane_label(facts["labels"][DISPLAY_NAME].as_str())
}

impl Session {
    pub(super) fn rename_pane(
        &mut self,
        params: actions::PaneRenameParams,
    ) -> io::Result<actions::ActionResult> {
        if !supports_rename(&self.core.hello) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Core does not support terminal labels",
            ));
        }
        let id = self.target(Some(&params.pane_id))?;
        let address = self.model.records[&id].address.clone();
        let label = crate::app::manual_pane_label(params.label.as_deref());
        // Existing Core contract: an empty value removes this key. No input,
        // program identity change, document CAS, PTY action or local persistence.
        let facts = self.core.request(
            LABEL_METHOD,
            json!({
                "terminal": address, "labels": {DISPLAY_NAME: label.unwrap_or_default()}
            }),
        )?;
        if facts["address"] != address || facts["labels"]["herdr"] != "tui" {
            return Err(invalid(
                "Core renamed terminal identity or membership changed",
            ));
        }
        self.model.insert(facts)?;
        self.snapshot_dirty = true;
        Ok(actions::ActionResult::PaneInfo {
            pane: self.pane_info(id)?,
        })
    }
}
