//! Client projection of Core-backed organization; program addresses stay opaque.
use super::{
    organization::{self, Organization},
    tab_layout::TabLayout,
};
use crate::{actions, layout::PaneId};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io,
};

pub(super) const MAX_PANES: usize = 128;

pub(super) struct Record {
    pub address: String,
    pub facts: Value,
    pub seq: u64,
}

pub(super) struct Model {
    pub boot: String,
    pub revision: u64,
    pub records: HashMap<PaneId, Record>,
    pub organization: Organization<TabLayout>,
    pub active_workspace: String,
    pub active_tabs: HashMap<String, String>,
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) fn status(facts: &Value) -> actions::AgentStatus {
    match facts["status"]["state"].as_str() {
        Some("working") => actions::AgentStatus::Working,
        Some("blocked" | "error") => actions::AgentStatus::Blocked,
        Some("idle") => actions::AgentStatus::Idle,
        Some("done") => actions::AgentStatus::Done,
        _ => actions::AgentStatus::Unknown,
    }
}

/// Only definitive program termination closes membership. A disconnected stream,
/// pending restart, server stop or missing record is not evidence of shell exit.
pub(super) fn program_completed(facts: &Value) -> bool {
    facts["state"] == "finished"
        && matches!(
            facts["reason"].as_str(),
            Some("exited" | "killed" | "signaled")
        )
}

impl Model {
    pub fn new(boot: String) -> Self {
        Self {
            boot,
            revision: 1,
            records: HashMap::new(),
            organization: Organization {
                workspaces: Vec::new(),
            },
            active_workspace: String::new(),
            active_tabs: HashMap::new(),
        }
    }
    pub fn id(&self, address: &str) -> Option<PaneId> {
        self.records
            .iter()
            .find_map(|(id, r)| (r.address == address).then_some(*id))
    }
    pub fn active_tab(&self) -> Option<&str> {
        self.active_tabs
            .get(&self.active_workspace)
            .map(String::as_str)
    }
    pub fn layout(&self) -> Option<&TabLayout> {
        self.workspace_layout(&self.active_workspace)
    }
    pub fn workspace_layout(&self, id: &str) -> Option<&TabLayout> {
        let active = self.active_tabs.get(id)?;
        self.organization
            .workspaces
            .iter()
            .find(|w| w.id == id)?
            .tabs
            .iter()
            .find(|t| &t.id == active)
            .map(|t| &t.panes)
    }
    pub fn tab_layout(&self, tab: &str) -> Option<&TabLayout> {
        self.all_layouts()
            .find_map(|(id, layout)| (id == tab).then_some(layout))
    }
    pub fn tab_layout_mut(&mut self, tab: &str) -> Option<&mut TabLayout> {
        self.organization
            .workspaces
            .iter_mut()
            .flat_map(|w| &mut w.tabs)
            .find(|t| t.id == tab)
            .map(|t| &mut t.panes)
    }
    pub fn focused(&self) -> Option<PaneId> {
        self.layout().map(TabLayout::focused)
    }
    pub fn context(
        &self,
        id: PaneId,
    ) -> Option<(
        &organization::Workspace<TabLayout>,
        &organization::Tab<TabLayout>,
    )> {
        self.organization.workspaces.iter().find_map(|w| {
            w.tabs
                .iter()
                .find(|t| t.panes.pane_ids().contains(&id))
                .map(|t| (w, t))
        })
    }
    pub fn all_layouts(&self) -> impl Iterator<Item = (&str, &TabLayout)> {
        self.organization
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .map(|t| (t.id.as_str(), &t.panes))
    }
    pub fn focus_workspace(&mut self, id: &str) -> io::Result<()> {
        if !self.organization.workspaces.iter().any(|w| w.id == id) {
            return Err(invalid("Core workspace not found"));
        }
        self.active_workspace = id.into();
        self.revision += 1;
        Ok(())
    }
    pub fn focus_tab(&mut self, id: &str) -> io::Result<()> {
        let workspace = self
            .organization
            .workspaces
            .iter()
            .find(|w| w.tabs.iter().any(|t| t.id == id))
            .ok_or_else(|| invalid("Core tab not found"))?;
        self.active_workspace = workspace.id.clone();
        self.active_tabs.insert(workspace.id.clone(), id.into());
        self.revision += 1;
        Ok(())
    }
    pub fn focus(&mut self, id: PaneId) -> io::Result<()> {
        let tab = self
            .context(id)
            .map(|(_, t)| t.id.clone())
            .ok_or_else(|| invalid("pane not found"))?;
        self.focus_tab(&tab)?;
        if let Some(layout) = self.tab_layout_mut(&tab) {
            layout.focus_pane(id);
        }
        Ok(())
    }
    pub fn remove(&mut self, id: PaneId) {
        let Some(tab) = self.context(id).map(|(_, t)| t.id.clone()) else {
            return;
        };
        // The original closed workspaces by index and selected the workspace that
        // took the closed one's position, clamped to the last. Record that
        // position before anything shifts.
        let active_index = self
            .organization
            .workspaces
            .iter()
            .position(|w| w.id == self.active_workspace);
        if self
            .tab_layout(&tab)
            .is_some_and(|l| l.pane_ids().len() == 1)
        {
            for workspace in &mut self.organization.workspaces {
                workspace.tabs.retain(|t| t.id != tab);
            }
            self.organization.workspaces.retain(|w| !w.tabs.is_empty());
        } else if let Some(layout) = self.tab_layout_mut(&tab) {
            layout.remove(id);
        }
        self.records.remove(&id);
        self.repair_focus(active_index);
        self.revision += 1;
    }
    fn repair_focus(&mut self, active_index: Option<usize>) {
        self.active_tabs
            .retain(|key, _| self.organization.workspaces.iter().any(|w| &w.id == key));
        for w in &self.organization.workspaces {
            if !self
                .active_tabs
                .get(&w.id)
                .is_some_and(|id| w.tabs.iter().any(|t| &t.id == id))
            {
                if let Some(tab) = w.tabs.first() {
                    self.active_tabs.insert(w.id.clone(), tab.id.clone());
                }
            }
        }
        if self
            .organization
            .workspaces
            .iter()
            .any(|w| w.id == self.active_workspace)
        {
            return;
        }
        // Original `close_workspaces`: keep the position, clamp into range. That
        // selects the workspace that moved into the closed slot, or the new last
        // one when the closed workspace was last.
        let len = self.organization.workspaces.len();
        self.active_workspace = if len == 0 {
            String::new()
        } else {
            self.organization.workspaces[active_index.unwrap_or(0).min(len - 1)]
                .id
                .clone()
        };
    }

    /// Install the validated App projection, preserving surviving records and focus.
    pub(super) fn install_projection(
        &mut self,
        doc: Organization<Vec<String>>,
        facts: &HashMap<String, Value>,
    ) -> io::Result<()> {
        let ids: HashMap<_, _> = self
            .records
            .iter()
            .map(|(&id, record)| (record.address.as_str(), id))
            .collect();
        let mut retained = HashSet::new();
        let mut added = Vec::new();
        let organization = doc.resolve(|tab, addresses| {
            let panes = addresses
                .into_iter()
                .map(|address| {
                    if let Some(&id) = ids.get(address.as_str()) {
                        retained.insert(id);
                        return Ok(id);
                    }
                    let facts = facts
                        .get(&address)
                        .ok_or_else(|| invalid(format!("Core facts missing for {address}")))?
                        .clone();
                    let id = PaneId::alloc();
                    added.push((
                        id,
                        Record {
                            address,
                            facts,
                            seq: 0,
                        },
                    ));
                    Ok(id)
                })
                .collect::<io::Result<Vec<_>>>()?;
            TabLayout::new(panes, self.tab_layout(tab).map(TabLayout::focused))
        })?;

        // Nothing in the model changes until the new projection is complete.
        let active_index = self
            .organization
            .workspaces
            .iter()
            .position(|w| w.id == self.active_workspace);
        self.records.retain(|id, _| retained.contains(id));
        self.records.extend(added);
        self.organization = organization;
        self.repair_focus(active_index);
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
include!("model_tests.rs");
