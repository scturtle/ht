//! Client projection of Core-backed organization; program addresses stay opaque.
use super::organization::{self, Branch, Organization};
use crate::{
    actions,
    layout::{Node, PaneId, TileLayout},
};
use ratatui::layout::Direction;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io,
};

pub(super) const WORKSPACE: &str = "core";
pub(super) const TAB: &str = "core.1";
pub(super) const MAX_PANES: usize = 128;

#[derive(Clone)]
pub(super) struct Record {
    pub address: String,
    pub facts: Value,
    pub seq: u64,
}

#[derive(Clone)]
pub(super) struct Model {
    pub boot: String,
    pub revision: u64,
    pub records: HashMap<PaneId, Record>,
    pub organization: Organization,
    pub layouts: HashMap<String, TileLayout>,
    pub active_workspace: String,
    pub active_tabs: HashMap<String, String>,
    /// Client presentation only; never serialized into the shared BSP document.
    pub zoomed_tabs: HashSet<String>,
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
            organization: Organization::default(),
            layouts: HashMap::new(),
            active_workspace: String::new(),
            active_tabs: HashMap::new(),
            zoomed_tabs: HashSet::new(),
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
    pub fn layout(&self) -> Option<&TileLayout> {
        self.active_tab().and_then(|tab| self.layouts.get(tab))
    }
    pub fn focused(&self) -> Option<PaneId> {
        self.layout().map(TileLayout::focused)
    }
    pub fn context(&self, id: PaneId) -> Option<(&organization::Workspace, &organization::Tab)> {
        self.organization.workspaces.iter().find_map(|w| {
            w.tabs.iter().find_map(|t| {
                self.layouts
                    .get(&t.id)
                    .filter(|l| l.pane_ids().contains(&id))
                    .map(|_| (w, t))
            })
        })
    }
    pub fn all_layouts(&self) -> impl Iterator<Item = (&str, &TileLayout)> {
        self.organization
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .filter_map(|t| self.layouts.get(&t.id).map(|l| (t.id.as_str(), l)))
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
        if let Some(layout) = self.layouts.get_mut(&tab) {
            layout.focus_pane(id);
        }
        Ok(())
    }
    fn record(&self, facts: Value) -> io::Result<Record> {
        let address = facts["address"]
            .as_str()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid("missing Core address"))?
            .to_owned();
        if self.organization.retiring.contains(&address) {
            return Err(invalid("Core terminal is retiring"));
        }
        if self.id(&address).is_some() {
            return Err(invalid("Core terminal already belongs to a tab"));
        }
        if self.records.len() >= MAX_PANES {
            return Err(invalid("Core TUI supports at most 128 panes"));
        }
        Ok(Record {
            address,
            facts,
            seq: 0,
        })
    }
    pub fn insert(&mut self, facts: Value) -> io::Result<PaneId> {
        self.insert_at(facts, self.focused(), Direction::Horizontal, 0.5, true)
    }
    pub fn insert_at(
        &mut self,
        facts: Value,
        target: Option<PaneId>,
        direction: Direction,
        ratio: f32,
        focus: bool,
    ) -> io::Result<PaneId> {
        if let Some(id) = facts["address"].as_str().and_then(|a| self.id(a)) {
            if let Some(r) = self.records.get_mut(&id) {
                r.facts = facts;
            }
            self.revision += 1;
            return Ok(id);
        }
        if self.organization.workspaces.is_empty() {
            // The original first workspace had no custom name either: it took its
            // name from the root pane's cwd.
            let (_, id) = self.create_workspace(facts, None, true)?;
            return Ok(id);
        }
        if !ratio.is_finite() || !(0.05..=0.95).contains(&ratio) {
            return Err(invalid("invalid Core split ratio"));
        }
        let target = target.ok_or_else(|| invalid("split target missing"))?;
        let tab = self
            .context(target)
            .map(|(_, t)| t.id.clone())
            .ok_or_else(|| invalid("split target not found"))?;
        let record = self.record(facts)?;
        let id = self
            .layouts
            .get_mut(&tab)
            .and_then(|l| l.split_pane(target, direction, ratio))
            .ok_or_else(|| invalid("split target not found"))?;
        self.records.insert(id, record);
        self.zoomed_tabs.remove(&tab);
        if focus {
            self.focus(id)?;
        }
        self.revision += 1;
        Ok(id)
    }
    pub fn create_workspace(
        &mut self,
        facts: Value,
        label: Option<String>,
        focus: bool,
    ) -> io::Result<(String, PaneId)> {
        organization::validate_label(label.as_deref())?;
        let number = self.organization.next_workspace;
        let next = number
            .checked_add(1)
            .ok_or_else(|| invalid("workspace identity exhausted"))?;
        let key = if number == 1 {
            WORKSPACE.into()
        } else {
            format!("core.w{number}")
        };
        let tab_key = if number == 1 {
            TAB.into()
        } else {
            format!("{key}.t1")
        };
        let record = self.record(facts)?;
        let terminal = record.address.clone();
        let (layout, id) = TileLayout::new();
        self.records.insert(id, record);
        self.organization.next_workspace = next;
        self.organization.workspaces.push(organization::Workspace {
            id: key.clone(),
            number,
            // Original `WorkspaceCreate`: a supplied label becomes the custom name;
            // otherwise the name is derived from the root pane cwd at display time.
            label: label.filter(|s| !s.is_empty()).unwrap_or_default(),
            next_tab: 2,
            tabs: vec![organization::Tab {
                id: tab_key.clone(),
                number: 1,
                label: "1".into(),
                custom_label: Some(false),
                root_terminal: Some(terminal.clone()),
                root: Branch::Pane { terminal },
            }],
        });
        self.layouts.insert(tab_key.clone(), layout);
        self.active_tabs.insert(key.clone(), tab_key);
        if focus || self.active_workspace.is_empty() {
            self.active_workspace = key.clone();
        }
        self.revision += 1;
        Ok((key, id))
    }
    pub fn create_tab(
        &mut self,
        workspace: &str,
        facts: Value,
        label: Option<String>,
        focus: bool,
    ) -> io::Result<(String, PaneId)> {
        organization::validate_label(label.as_deref())?;
        let index = self
            .organization
            .workspaces
            .iter()
            .position(|w| w.id == workspace)
            .ok_or_else(|| invalid("Core workspace not found"))?;
        let number = self.organization.workspaces[index].next_tab;
        let next = number
            .checked_add(1)
            .ok_or_else(|| invalid("tab identity exhausted"))?;
        let key = format!("{workspace}.t{number}");
        let record = self.record(facts)?;
        let terminal = record.address.clone();
        let (layout, id) = TileLayout::new();
        self.records.insert(id, record);
        let w = &mut self.organization.workspaces[index];
        w.next_tab = next;
        w.tabs.push(organization::Tab {
            id: key.clone(),
            number,
            custom_label: Some(label.as_ref().is_some_and(|s| !s.is_empty())),
            label: label
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| organization::Tab::default_label(w.tabs.len())),
            root_terminal: Some(terminal.clone()),
            root: Branch::Pane { terminal },
        });
        self.layouts.insert(key.clone(), layout);
        if focus {
            self.focus_tab(&key)?;
        }
        self.revision += 1;
        Ok((key, id))
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
        if self.layouts.get(&tab).is_some_and(|l| l.pane_count() == 1) {
            self.layouts.remove(&tab);
            for workspace in &mut self.organization.workspaces {
                workspace.tabs.retain(|t| t.id != tab);
            }
            self.organization.workspaces.retain(|w| !w.tabs.is_empty());
        } else if let Some(layout) = self.layouts.get_mut(&tab) {
            layout.close_pane(id);
        }
        self.records.remove(&id);
        self.zoomed_tabs.remove(&tab);
        self.repair_focus(active_index);
        self.revision += 1;
    }
    pub fn retire(&mut self, id: PaneId) -> io::Result<()> {
        let address = self
            .records
            .get(&id)
            .ok_or_else(|| invalid("Core pane not found"))?
            .address
            .clone();
        self.organization.retiring.insert(address);
        self.remove(id);
        Ok(())
    }
    pub fn retire_completed_programs(&mut self) -> io::Result<()> {
        let completed = self
            .records
            .iter()
            .filter(|(_, record)| program_completed(&record.facts))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in completed {
            self.retire(id)?;
        }
        Ok(())
    }
    pub fn close_tab(&mut self, key: &str) -> io::Result<Vec<String>> {
        let ids = self
            .layouts
            .get(key)
            .ok_or_else(|| invalid("Core tab not found"))?
            .pane_ids();
        let addresses = ids
            .iter()
            .map(|id| self.records[id].address.clone())
            .collect();
        for id in ids {
            self.retire(id)?;
        }
        Ok(addresses)
    }
    pub fn close_workspace(&mut self, key: &str) -> io::Result<Vec<String>> {
        let tabs = self
            .organization
            .workspaces
            .iter()
            .find(|w| w.id == key)
            .ok_or_else(|| invalid("Core workspace not found"))?
            .tabs
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        let mut addresses = Vec::new();
        for tab in tabs {
            addresses.extend(self.close_tab(&tab)?);
        }
        Ok(addresses)
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

    pub fn shared(&self) -> io::Result<Organization> {
        let mut doc = self.organization.clone();
        for w in &mut doc.workspaces {
            for t in &mut w.tabs {
                let layout = self
                    .layouts
                    .get(&t.id)
                    .ok_or_else(|| invalid("missing projected tab layout"))?;
                t.root = Branch::from_node(layout.root(), &|id| {
                    self.records
                        .get(&id)
                        .map(|r| r.address.clone())
                        .unwrap_or_default()
                });
            }
        }
        doc.validate()?;
        Ok(doc)
    }
    pub fn install(&mut self, doc: Organization, facts: &HashMap<String, Value>) -> io::Result<()> {
        doc.validate()?;
        if doc.next_workspace < self.organization.next_workspace
            || !self.organization.retiring.is_subset(&doc.retiring)
        {
            return Err(invalid(
                "shared counters and close-intent tombstones cannot move backwards",
            ));
        }
        for workspace in &doc.workspaces {
            if let Some(previous) = self
                .organization
                .workspaces
                .iter()
                .find(|w| w.id == workspace.id)
            {
                if workspace.number != previous.number || workspace.next_tab < previous.next_tab {
                    return Err(invalid("shared workspace identity or tab counter changed"));
                }
                for tab in &workspace.tabs {
                    match previous.tabs.iter().find(|t| t.id == tab.id) {
                        Some(old) if old.number != tab.number => {
                            return Err(invalid("shared tab identity changed"))
                        }
                        None if tab.number < previous.next_tab => {
                            return Err(invalid("closed tab number cannot be reused"))
                        }
                        _ => {}
                    }
                }
            } else if workspace.number < self.organization.next_workspace {
                return Err(invalid("closed workspace number cannot be reused"));
            }
        }
        let mut candidate = self.clone();
        let wanted = doc
            .addresses()
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        candidate.records.retain(|_, r| wanted.contains(&r.address));
        for address in &wanted {
            if candidate.id(address).is_none() {
                let fact = facts
                    .get(address)
                    .ok_or_else(|| invalid(format!("Core facts missing for {address}")))?;
                let id = PaneId::alloc();
                candidate.records.insert(
                    id,
                    Record {
                        address: address.clone(),
                        facts: fact.clone(),
                        seq: 0,
                    },
                );
            }
        }
        let mut layouts = HashMap::new();
        for w in &doc.workspaces {
            for t in &w.tabs {
                let root = t.root.node(&|address| {
                    candidate
                        .id(address)
                        .ok_or_else(|| invalid("Core pane identity missing"))
                })?;
                let mut addresses = Vec::new();
                t.root.addresses(&mut addresses);
                let focus = self
                    .layouts
                    .get(&t.id)
                    .map(TileLayout::focused)
                    .filter(|id| {
                        candidate
                            .records
                            .get(id)
                            .is_some_and(|r| addresses.contains(&r.address.as_str()))
                    })
                    .or_else(|| addresses.first().and_then(|a| candidate.id(a)))
                    .ok_or_else(|| invalid("tab has no focus"))?;
                layouts.insert(t.id.clone(), TileLayout::from_saved(root, focus));
            }
        }
        // Native Tab clears zoom on insertion/removal, not focus, swaps or ratios.
        // Apply the same rule to changes received from another client.
        candidate
            .zoomed_tabs
            .retain(|tab| match (self.layouts.get(tab), layouts.get(tab)) {
                (Some(old), Some(new)) => {
                    let old_ids = old.pane_ids();
                    let new_ids = new.pane_ids();
                    old_ids.len() == new_ids.len() && old_ids.iter().all(|id| new_ids.contains(id))
                }
                _ => false,
            });
        candidate.layouts = layouts;
        candidate.organization = doc;
        // A document from another client may drop the active workspace. Keep the
        // current position rather than jumping to the first one.
        let active_index = self
            .organization
            .workspaces
            .iter()
            .position(|w| w.id == self.active_workspace);
        candidate.repair_focus(active_index);
        candidate.revision += 1;
        *self = candidate;
        Ok(())
    }
    /// One-time migration of the private experimental layout, never shared focus.
    pub fn restore(&mut self, saved: &actions::LayoutDescription) -> io::Result<()> {
        fn convert(
            node: &actions::LayoutNode,
            model: &Model,
            seen: &mut HashSet<PaneId>,
        ) -> io::Result<Option<Node>> {
            Ok(match node {
                actions::LayoutNode::Pane { pane } => {
                    let Some(id) = pane.pane_id.as_deref().and_then(|a| model.id(a)) else {
                        return Ok(None);
                    };
                    if !seen.insert(id) {
                        return Err(invalid("duplicate terminal in saved Core layout"));
                    }
                    Some(Node::Pane(id))
                }
                actions::LayoutNode::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => {
                    if !ratio.is_finite() || !(0.05..=0.95).contains(ratio) {
                        return Err(invalid("invalid saved Core split ratio"));
                    }
                    match (convert(first, model, seen)?, convert(second, model, seen)?) {
                        (Some(first), Some(second)) => Some(Node::Split {
                            direction: if *direction == actions::SplitDirection::Right {
                                Direction::Horizontal
                            } else {
                                Direction::Vertical
                            },
                            ratio: *ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        }),
                        (one, None) | (None, one) => one,
                    }
                }
            })
        }
        let mut seen = HashSet::new();
        if let Some(root) = convert(&saved.root, self, &mut seen)? {
            let focus = self
                .id(&saved.focused_pane_id)
                .filter(|id| seen.contains(id))
                .or_else(|| seen.iter().copied().min_by_key(|id| id.raw()))
                .ok_or_else(|| invalid("saved layout has no focus"))?;
            let mut layout = TileLayout::from_saved(root, focus);
            let mut extra = self
                .layout()
                .map(TileLayout::pane_ids)
                .unwrap_or_default()
                .into_iter()
                .filter(|id| !seen.contains(id))
                .collect::<Vec<_>>();
            extra.sort_by_key(|id| id.raw());
            for id in extra {
                layout.insert_pane_near(focus, id, Direction::Horizontal, 0.5, false);
            }
            let tab = self
                .active_tab()
                .ok_or_else(|| invalid("no Core tab"))?
                .to_owned();
            if saved.zoomed && layout.pane_count() > 1 {
                self.zoomed_tabs.insert(tab.clone());
            } else {
                self.zoomed_tabs.remove(&tab);
            }
            self.layouts.insert(tab, layout);
        }
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
include!("model_tests.rs");
