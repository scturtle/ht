//! Core document reads and local view preferences.
use super::{
    model::{invalid, Model},
    organization::Organization,
    transport::CoreRelay,
};
use serde_json::{json, Value};
use std::{collections::HashMap, io, path::Path};

pub(super) trait DocumentRpc {
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value>;
}
impl DocumentRpc for CoreRelay {
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        CoreRelay::request(self, method, params)
    }
}

pub(super) fn hydrate(
    core: &mut impl DocumentRpc,
    model: &Model,
    doc: &Organization<Vec<String>>,
    facts: &mut HashMap<String, Value>,
) -> io::Result<()> {
    for address in doc.addresses() {
        if model.id(address).is_none() && !facts.contains_key(address) {
            let record = match core.request("terminal.get", json!({"terminal":address})) {
                Ok(record) => record,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Only definitive absence is grounds to omit an unread pane.
                    facts.insert(
                        address.into(),
                        json!({"address":address,"reason":"removed"}),
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            if record["address"] != address {
                return Err(invalid("App terminal identity changed"));
            }
            facts.insert(address.into(), record);
        }
    }
    Ok(())
}

/// UI revisions restart when a relay rehydrates the document, even when Core
/// and every program survive. Give that projection a fresh scope so old frames,
/// leases and uncertain API replies cannot be mistaken for the new stream.
pub(super) fn projection_boot() -> io::Result<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let epoch = NEXT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| invalid("Core client projection epochs exhausted"))?;
    Ok(format!("core-client-{}-{epoch}", std::process::id()))
}

pub(super) fn restore_preferences(path: &Path, model: &mut Model) -> io::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if bytes.len() > 128 * 1024 {
        return Err(invalid("Core client preferences exceed 128 KiB"));
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if let Some(tabs) = value["active_tabs"].as_object() {
        for tab in tabs.values().filter_map(Value::as_str) {
            if let Some(workspace) = model
                .organization
                .workspaces
                .iter()
                .find(|w| w.tabs.iter().any(|t| t.id == tab))
            {
                model.active_tabs.insert(workspace.id.clone(), tab.into());
            }
        }
    }
    if let Some(points) = value["focus"].as_object() {
        for address in points.values().filter_map(Value::as_str) {
            if let Some(id) = model.id(address) {
                if let Some(tab) = model.context(id).map(|(_, t)| t.id.clone()) {
                    if let Some(layout) = model.tab_layout_mut(&tab) {
                        layout.focus_pane(id);
                    }
                }
            }
        }
    }
    if let Some(workspace) = value["active_workspace"].as_str() {
        let _ = model.focus_workspace(workspace);
    }
    Ok(())
}
