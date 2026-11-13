//! Shared terminal metadata through existing Core APIs, not TUI-owned storage.
use super::{model::invalid, session::Session};
use crate::actions;
use serde_json::{json, Value};
use std::io;

const LABEL_METHOD: &str = "terminal.label";
const DISPLAY_NAME: &str = "herdr.display_name";

/// Public Core snapshots expose the current cwd under foreground, not the
/// launch cwd. Event-derived last_cwd survives a later null foreground.
pub(super) fn current_cwd(facts: &Value) -> Option<&str> {
    facts["foreground"]["cwd"]
        .as_str()
        .or_else(|| facts["last_cwd"].as_str())
        .or_else(|| facts["cwd"].as_str())
}

pub(super) fn supports_rename(hello: &Value) -> bool {
    hello["methods"]
        .as_array()
        .is_some_and(|methods| methods.iter().any(|m| m == LABEL_METHOD))
}

pub(super) fn pane_label(facts: &Value) -> Option<String> {
    manual_pane_label(facts["labels"][DISPLAY_NAME].as_str())
}

/// A blank manual label restores automatic presentation.
fn manual_pane_label(label: Option<&str>) -> Option<String> {
    label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
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
        let label = manual_pane_label(params.label.as_deref());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_cwd_prefers_public_foreground_and_preserves_event_fallbacks() {
        for (facts, expected) in [
            (
                json!({"cwd":"/launch","last_cwd":"/stale","foreground":{"cwd":"/actual"}}),
                Some("/actual"),
            ),
            (
                json!({"cwd":"/launch","last_cwd":"/last","foreground":null}),
                Some("/last"),
            ),
            (json!({"cwd":"/launch","foreground":null}), Some("/launch")),
            (
                json!({"foreground":{"pid":12},"cwd":"/launch"}),
                Some("/launch"),
            ),
            (json!({}), None),
        ] {
            assert_eq!(current_cwd(&facts), expected, "{facts}");
        }
    }

    #[test]
    fn blank_manual_names_restore_automatic_labels() {
        for label in [None, Some(""), Some(" \t\n"), Some("\u{3000}")] {
            assert_eq!(manual_pane_label(label), None);
        }
        assert_eq!(
            manual_pane_label(Some("  中文 pane  ")).as_deref(),
            Some("中文 pane")
        );
        assert_eq!(
            pane_label(&json!({"labels":{DISPLAY_NAME:"  custom  "}})).as_deref(),
            Some("custom")
        );
    }
}
