//! Versioned Core document transactions and one-time private prototype migration.
use super::{
    model::{invalid, Model},
    organization::{Organization, DOCUMENT},
    session::Session,
    transport::Core,
};
use crate::protocol::ClientSurfaceSize;
use serde_json::{json, Value};
use std::{collections::HashMap, io, path::Path};

trait DocumentRpc {
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value>;
}
impl DocumentRpc for Core {
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        Core::request(self, method, params)
    }
}

fn read(core: &mut impl DocumentRpc) -> io::Result<(u64, Option<Organization>)> {
    let value = core.request("doc.get", json!({"name":DOCUMENT}))?;
    let version = value["version"]
        .as_u64()
        .ok_or_else(|| invalid("Core document has no version"))?;
    if value["value"].is_null() {
        if version != 0 {
            return Err(invalid(
                "shared organization was deleted; refusing to reseed it implicitly",
            ));
        }
        return Ok((version, None));
    }
    Ok((version, Some(Organization::decode(value["value"].clone())?)))
}

fn write(
    core: &mut impl DocumentRpc,
    version: u64,
    organization: &Organization,
) -> io::Result<u64> {
    organization.validate()?;
    let result = core.request(
        "doc.set",
        json!({"name":DOCUMENT,"version":version,"value":organization}),
    )?;
    result["version"]
        .as_u64()
        .filter(|v| version.checked_add(1) == Some(*v))
        .ok_or_else(|| invalid("Core document write returned an invalid version"))
}

fn hydrate(
    core: &mut impl DocumentRpc,
    model: &Model,
    doc: &Organization,
    facts: &mut HashMap<String, Value>,
) -> io::Result<()> {
    for address in doc.addresses() {
        if model.id(address).is_none() && !facts.contains_key(address) {
            let record = match core.request("terminal.get", json!({"terminal":address})) {
                Ok(record) => record,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // The Core explicitly confirmed absence; retain a non-running
                    // placeholder until the user closes this shared membership.
                    facts.insert(
                        address.into(),
                        json!({"address":address,"reason":"removed"}),
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            if record["address"] != address || record["labels"]["herdr"] != "tui" {
                return Err(invalid(
                    "shared document references a terminal outside this session",
                ));
            }
            facts.insert(address.into(), record);
        }
    }
    Ok(())
}

pub(super) fn bootstrap(
    core: &mut Core,
    subscription: &Value,
    size: ClientSurfaceSize,
    home: &Path,
    shell: &[String],
) -> io::Result<(Model, u64)> {
    // Subscription is established BEFORE the document read. Terminal-filtered
    // subscriptions cannot receive Core's document invalidation events.
    let terminals = subscription["terminals"]
        .as_array()
        .ok_or_else(|| invalid("Core subscription lacks terminals"))?;
    let mut facts = terminals
        .iter()
        .filter_map(|v| v["address"].as_str().map(|a| (a.to_owned(), v.clone())))
        .collect::<HashMap<_, _>>();
    let mut model = Model::new(format!("core-client-{}", std::process::id()));
    let (mut version, document) = read(core)?;
    let document = if let Some(document) = document {
        document
    } else {
        // Adopt only tagged terminals, and only while seeding an absent document.
        // Once membership exists, live Core records never invent membership.
        for terminal in terminals {
            if terminal["labels"]["herdr"] == "tui" && terminal["state"] == "live" {
                model.insert(terminal.clone())?;
            }
        }
        if model.records.is_empty() {
            let record = core.request("terminal.run", json!({"argv":shell,"labels":{"herdr":"tui"},"no_parent":true,
                "cwd":std::env::current_dir()?.to_string_lossy(),"size":{"cols":size.cols.max(4),"rows":size.rows.max(2)},
                "env":{"TERM":crate::pane::PANE_TERM,"COLORTERM":"truecolor"}}))?;
            if let Some(address) = record["address"].as_str() {
                facts.insert(address.into(), record.clone());
            }
            model.insert(record)?;
        }
        match std::fs::read(home.join("layout.json")) {
            Ok(bytes) => {
                if bytes.len() > 128 * 1024 {
                    return Err(invalid("legacy Core layout exceeds 128 KiB"));
                }
                model.restore(&serde_json::from_slice(&bytes)?)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let candidate = model.shared()?;
        match write(core, 0, &candidate) {
            Ok(v) => {
                version = v;
                candidate
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let (v, document) = read(core)?;
                version = v;
                // Do not kill a seed program after a race or ambiguous publication.
                // It stays inspectable in Core; it is never silently adopted later.
                document
                    .ok_or_else(|| invalid("competing organization initialization disappeared"))?
            }
            Err(e) => return Err(e),
        }
    };
    hydrate(core, &model, &document, &mut facts)?;
    model.install(document, &facts)?;
    Ok((model, version))
}

pub(super) fn restore_preferences(home: &Path, model: &mut Model) -> io::Result<()> {
    let bytes = match std::fs::read(home.join("preferences.json")) {
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
                    if let Some(layout) = model.layouts.get_mut(&tab) {
                        layout.focus_pane(id);
                    }
                }
            }
        }
    }
    if let Some(tabs) = value["zoomed_tabs"].as_array() {
        model.zoomed_tabs = tabs
            .iter()
            .filter_map(Value::as_str)
            .filter(|tab| model.layouts.get(*tab).is_some_and(|l| l.pane_count() > 1))
            .map(str::to_owned)
            .collect();
    }
    if let Some(workspace) = value["active_workspace"].as_str() {
        let _ = model.focus_workspace(workspace);
    }
    Ok(())
}

fn refresh(core: &mut impl DocumentRpc, model: &mut Model, version: &mut u64) -> io::Result<bool> {
    let (next_version, doc) = read(core)?;
    if next_version == *version {
        return Ok(false);
    }
    if next_version < *version {
        return Err(invalid("Core document version moved backwards"));
    }
    let doc = doc.ok_or_else(|| invalid("shared organization disappeared"))?;
    let mut facts = HashMap::new();
    hydrate(core, model, &doc, &mut facts)?;
    model.install(doc, &facts)?;
    *version = next_version;
    Ok(true)
}

fn transact<T>(
    core: &mut impl DocumentRpc,
    model: &mut Model,
    version: &mut u64,
    mut apply: impl FnMut(&mut Model) -> io::Result<T>,
) -> io::Result<T> {
    for _ in 0..4 {
        refresh(core, model, version)?;
        let mut candidate = model.clone();
        let result = apply(&mut candidate)?;
        let doc = candidate.shared()?;
        if doc == model.shared()? {
            // Read-only/no-op results still reflect the freshly reconciled model.
            // Preserve any legitimate client-local focus change, but never
            // publish an identical document or manufacture a projection revision.
            *model = candidate;
            return Ok(result);
        }
        candidate.revision = candidate.revision.saturating_add(1);
        match write(core, *version, &doc) {
            Ok(next_version) => {
                candidate.organization = doc;
                *model = candidate;
                *version = next_version;
                return Ok(result);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                // An absent reply does not prove non-commit. Refetch when possible;
                // never roll back membership or kill a potentially published program.
                if let Err(error) = refresh(core, model, version) {
                    tracing::warn!(%error, "reconciling uncertain Core document write failed");
                }
                return Err(e);
            }
        }
    }
    if let Err(error) = refresh(core, model, version) {
        tracing::warn!(%error, "refreshing after Core CAS contention failed");
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "shared organization changed repeatedly; retry the action",
    ))
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;

impl Session {
    pub(super) fn refresh_shared(&mut self) -> io::Result<bool> {
        let changed = refresh(&mut self.core, &mut self.model, &mut self.document_version)?;
        if changed {
            self.dirty = true;
            self.snapshot_dirty = true;
        }
        Ok(changed)
    }
    pub(super) fn change<T>(
        &mut self,
        apply: impl FnMut(&mut Model) -> io::Result<T>,
    ) -> io::Result<T> {
        let before = self.model.revision;
        let result = transact(
            &mut self.core,
            &mut self.model,
            &mut self.document_version,
            apply,
        );
        if self.model.revision != before {
            self.dirty = true;
            self.snapshot_dirty = true;
        }
        result
    }

    pub(super) fn retire_completed_programs(&mut self) -> io::Result<()> {
        if !self
            .model
            .records
            .values()
            .any(|r| super::model::program_completed(&r.facts))
        {
            return Ok(());
        }
        // CAS-only: completion is already confirmed; never run/kill/replay input.
        self.change(Model::retire_completed_programs)?;
        self.sync_views()?;
        self.save()
    }

    pub(super) fn finish_retirements(&mut self, addresses: &[String]) -> io::Result<()> {
        let mut failed = Vec::new();
        for address in addresses {
            let result = (|| -> io::Result<()> {
                let record = match self
                    .core
                    .request("terminal.get", json!({"terminal":address}))
                {
                    Ok(record) => record,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(e),
                };
                if record["address"] != address.as_str() {
                    return Err(invalid("Core close identity changed"));
                }
                if record["state"] != "live" || record["labels"]["herdr"] != "tui" {
                    return Ok(());
                }
                self.core
                    .request("terminal.kill", json!({"terminal":address}))?;
                Ok(())
            })();
            if let Err(error) = result {
                tracing::warn!(%address, %error, "Core close intent saved; termination requires reconciliation");
                failed.push(address.as_str());
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "close intent committed; program termination still pending: {}",
                failed.join(", ")
            )))
        }
    }
}
