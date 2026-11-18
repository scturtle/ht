//! Existing endpoint/TUI projections of the shared organization and local focus.
use super::{
    model::{invalid, status, Model},
    organization::{Tab, Workspace},
    tab_layout::TabLayout,
};
use crate::{
    actions::{self, AgentStatus},
    protocol::*,
};
use std::io;

fn higher_priority_status(left: AgentStatus, right: AgentStatus) -> AgentStatus {
    std::cmp::max_by_key(left, right, |state| match state {
        AgentStatus::Blocked => 4,
        AgentStatus::Working => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Done => 1,
        AgentStatus::Unknown => 0,
    })
}

fn aggregate(values: impl Iterator<Item = AgentStatus>) -> AgentStatus {
    values.fold(AgentStatus::Unknown, higher_priority_status)
}

impl Model {
    fn tab_status(&self, tab: &Tab<TabLayout>) -> AgentStatus {
        aggregate(
            tab.panes
                .pane_ids()
                .iter()
                .filter_map(|id| self.records.get(id))
                .map(|r| status(&r.facts)),
        )
    }

    fn workspace_name(&self, workspace: &Workspace<TabLayout>) -> String {
        if !workspace.label.is_empty() {
            return workspace.label.clone();
        }
        workspace
            .tabs
            .first()
            .and_then(|tab| tab.panes.pane_ids().first())
            .and_then(|pane| self.records.get(pane))
            .and_then(|record| super::metadata::current_cwd(&record.facts))
            .map_or_else(
                || "workspace".into(),
                |cwd| crate::client::identity::derive_label_from_cwd(std::path::Path::new(cwd)),
            )
    }

    pub fn workspace_info(&self, key: &str) -> io::Result<actions::WorkspaceInfo> {
        let (index, w) = self
            .organization
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, w)| w.id == key)
            .ok_or_else(|| invalid("Core workspace not found"))?;
        Ok(actions::WorkspaceInfo {
            workspace_id: w.id.clone(),
            // The original reported the workspace's position, not a counter.
            number: index + 1,
            label: self.workspace_name(w),
            focused: self.active_workspace == key,
            pane_count: w.tabs.iter().map(|t| t.panes.pane_ids().len()).sum(),
            tab_count: w.tabs.len(),
            active_tab_id: self.active_tabs.get(key).cloned().unwrap_or_default(),
            agent_status: aggregate(w.tabs.iter().map(|t| self.tab_status(t))),
            tokens: Default::default(),
            worktree: None,
        })
    }
    pub fn tab_info(&self, key: &str) -> io::Result<actions::TabInfo> {
        let (w, index, t) = self
            .organization
            .workspaces
            .iter()
            .find_map(|w| {
                w.tabs
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.id == key)
                    .map(|(i, t)| (w, i, t))
            })
            .ok_or_else(|| invalid("Core tab not found"))?;
        Ok(actions::TabInfo {
            tab_id: t.id.clone(),
            workspace_id: w.id.clone(),
            number: index + 1,
            label: t.display_name(index),
            focused: self.active_workspace == w.id && self.active_tab() == Some(key),
            pane_count: t.panes.pane_ids().len(),
            agent_status: self.tab_status(t),
        })
    }
    pub fn snapshot(&self) -> ClientShellSnapshot {
        let mut panes = Vec::new();
        let mut agents = Vec::new();
        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        for (position, workspace) in self.organization.workspaces.iter().enumerate() {
            let workspace_focused = workspace.id == self.active_workspace;
            let active_tab = self
                .active_tabs
                .get(&workspace.id)
                .map_or("", String::as_str);
            let mut workspace_status = AgentStatus::Unknown;
            for (index, tab) in workspace.tabs.iter().enumerate() {
                let tab_focused = workspace_focused && tab.id == active_tab;
                let mut tab_status = AgentStatus::Unknown;
                let focused_pane = tab_focused.then(|| tab.panes.focused());
                for &id in tab.panes.pane_ids() {
                    let Some(record) = self.records.get(&id) else {
                        continue;
                    };
                    let facts = &record.facts;
                    let focused = focused_pane == Some(id);
                    let cwd = facts["cwd"].as_str().map(str::to_owned);
                    panes.push(ClientShellPane {
                        pane_id: record.address.clone(),
                        workspace_id: workspace.id.clone(),
                        tab_id: tab.id.clone(),
                        label: super::metadata::pane_label(facts),
                        cwd,
                        foreground_cwd: super::metadata::current_cwd(facts).map(str::to_owned),
                        focused,
                        right_click_passthrough: false,
                    });
                    if facts["status"].is_object() {
                        let pane_status = status(facts);
                        tab_status = higher_priority_status(tab_status, pane_status);
                        agents.push(ClientShellAgent {
                            pane_id: record.address.clone(),
                            workspace_id: workspace.id.clone(),
                            tab_id: tab.id.clone(),
                            name: None,
                            display_agent: Some(
                                facts["status"]["app"]
                                    .as_str()
                                    .unwrap_or("program")
                                    .to_owned(),
                            ),
                            agent: None,
                            title: facts["status"]["message"].as_str().map(str::to_owned),
                            terminal_title: facts["title"].as_str().map(str::to_owned),
                            terminal_title_stripped: facts["title"].as_str().map(str::to_owned),
                            agent_status: pane_status,
                            state_change_seq: record.seq,
                            state_labels: vec![(
                                "OSC 7501".into(),
                                facts["status"]["state"]
                                    .as_str()
                                    .unwrap_or("unknown")
                                    .into(),
                            )],
                            tokens: Vec::new(),
                            focused,
                        });
                    }
                }
                workspace_status = higher_priority_status(workspace_status, tab_status);
                tabs.push(ClientShellTab {
                    tab_id: tab.id.clone(),
                    workspace_id: workspace.id.clone(),
                    number: index + 1,
                    label: tab.display_name(index),
                    custom_label: !tab.label.is_empty(),
                    focused: tab_focused,
                    agent_status: tab_status,
                });
            }
            workspaces.push(ClientShellWorkspace {
                workspace_id: workspace.id.clone(),
                active_tab_id: active_tab.into(),
                new_workspace_cwd: workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.id == active_tab)
                    .and_then(|tab| self.records.get(&tab.panes.focused()))
                    .and_then(|record| super::metadata::current_cwd(&record.facts))
                    .unwrap_or(".")
                    .into(),
                number: position + 1,
                label: self.workspace_name(workspace),
                custom_label: !workspace.label.is_empty(),
                branch: None,
                git_ahead_behind: None,
                tokens: Vec::new(),
                worktree: None,
                focused: workspace_focused,
                agent_status: workspace_status,
            });
        }
        ClientShellSnapshot {
            boot_id: self.boot.clone(),
            revision: self.revision,
            config_diagnostic: None,
            product_announcement: None,
            focused_workspace_id: (!self.active_workspace.is_empty())
                .then(|| self.active_workspace.clone()),
            focused_tab_id: self.active_tab().map(str::to_owned),
            focused_pane_id: self
                .focused()
                .and_then(|id| self.records.get(&id))
                .map(|r| r.address.clone()),
            agent_view_label: None,
            agent_order: Vec::new(),
            workspaces,
            tabs,
            panes,
            agents,
        }
    }
}
