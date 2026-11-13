//! Existing endpoint/TUI projections of the shared organization and local focus.
use super::model::{invalid, status, Model};
use crate::{actions, layout::Node, protocol::*};
use ratatui::layout::{Direction, Rect};
use std::io;

fn aggregate(values: impl Iterator<Item = actions::AgentStatus>) -> actions::AgentStatus {
    values
        .max_by_key(|state| match state {
            actions::AgentStatus::Blocked => 4,
            actions::AgentStatus::Working => 3,
            actions::AgentStatus::Idle => 2,
            actions::AgentStatus::Done => 1,
            _ => 0,
        })
        .unwrap_or(actions::AgentStatus::Unknown)
}

// Pure conversion retained from the original app/api/panes.rs.
impl From<Rect> for actions::PaneLayoutRect {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

impl Model {
    fn tab_status(&self, tab: &str) -> actions::AgentStatus {
        aggregate(
            self.layouts
                .get(tab)
                .into_iter()
                .flat_map(|l| l.pane_ids())
                .filter_map(|id| self.records.get(&id))
                .map(|r| status(&r.facts)),
        )
    }
    /// The original named a workspace after the cwd of its first tab's root pane.
    fn workspace_root_cwd(&self, w: &super::organization::Workspace) -> Option<String> {
        let tab = w.tabs.first()?;
        let address = tab.root_terminal.clone().or_else(|| {
            self.layouts
                .get(&tab.id)
                .and_then(|layout| super::view::first_leaf(layout.root()))
                .and_then(|id| self.records.get(&id))
                .map(|record| record.address.clone())
        })?;
        let record = self.records.get(&self.id(&address)?)?;
        super::metadata::current_cwd(&record.facts).map(str::to_owned)
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
            label: w.display_name(self.workspace_root_cwd(w).as_deref()),
            focused: self.active_workspace == key,
            pane_count: w
                .tabs
                .iter()
                .filter_map(|t| self.layouts.get(&t.id))
                .map(|l| l.pane_count())
                .sum(),
            tab_count: w.tabs.len(),
            active_tab_id: self.active_tabs.get(key).cloned().unwrap_or_default(),
            agent_status: aggregate(w.tabs.iter().map(|t| self.tab_status(&t.id))),
            tokens: Default::default(),
            worktree: None,
        })
    }
    pub fn tab_info(&self, key: &str) -> io::Result<actions::TabInfo> {
        let (w, t) = self
            .organization
            .workspaces
            .iter()
            .find_map(|w| w.tabs.iter().find(|t| t.id == key).map(|t| (w, t)))
            .ok_or_else(|| invalid("Core tab not found"))?;
        Ok(actions::TabInfo {
            tab_id: t.id.clone(),
            workspace_id: w.id.clone(),
            number: t.number,
            label: t.label.clone(),
            focused: self.active_workspace == w.id && self.active_tab() == Some(key),
            pane_count: self.layouts.get(key).map_or(0, |l| l.pane_count()),
            agent_status: self.tab_status(key),
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
                .cloned()
                .unwrap_or_default();
            workspaces.push(ClientShellWorkspace {
                workspace_id: workspace.id.clone(),
                active_tab_id: active_tab.clone(),
                new_workspace_cwd: self
                    .layouts
                    .get(&active_tab)
                    .and_then(|layout| self.records.get(&layout.focused()))
                    .and_then(|record| super::metadata::current_cwd(&record.facts))
                    .unwrap_or(".")
                    .into(),
                number: position + 1,
                label: workspace.display_name(self.workspace_root_cwd(workspace).as_deref()),
                custom_label: workspace.is_custom_named(),
                branch: None,
                git_ahead_behind: None,
                tokens: Vec::new(),
                worktree: None,
                focused: workspace_focused,
                agent_status: aggregate(workspace.tabs.iter().map(|t| self.tab_status(&t.id))),
            });
            for (index, tab) in workspace.tabs.iter().enumerate() {
                let tab_focused = workspace_focused && tab.id == active_tab;
                tabs.push(ClientShellTab {
                    tab_id: tab.id.clone(),
                    workspace_id: workspace.id.clone(),
                    number: tab.number,
                    label: tab.display_name(index),
                    custom_label: tab.is_custom_named(),
                    zoomed: self.zoomed_tabs.contains(&tab.id),
                    focused: tab_focused,
                    agent_status: self.tab_status(&tab.id),
                });
                let Some(layout) = self.layouts.get(&tab.id) else {
                    continue;
                };
                for id in layout.pane_ids() {
                    let Some(record) = self.records.get(&id) else {
                        continue;
                    };
                    let facts = &record.facts;
                    let focused = tab_focused && id == layout.focused();
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
                            agent_status: status(facts),
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
            }
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
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: String::new(),
            agent_view_label: None,
            agent_order: Vec::new(),
            workspaces,
            tabs,
            panes,
            agents,
        }
    }
    pub fn description(&self) -> Option<actions::LayoutDescription> {
        self.description_for(self.active_tab()?)
    }
    pub fn description_for(&self, tab: &str) -> Option<actions::LayoutDescription> {
        fn convert(node: &Node, model: &Model) -> actions::LayoutNode {
            match node {
                Node::Pane(id) => actions::LayoutNode::Pane {
                    pane: actions::LayoutPane {
                        pane_id: model.records.get(id).map(|r| r.address.clone()),
                        ..Default::default()
                    },
                },
                Node::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => actions::LayoutNode::Split {
                    direction: if *direction == Direction::Horizontal {
                        actions::SplitDirection::Right
                    } else {
                        actions::SplitDirection::Down
                    },
                    ratio: *ratio,
                    first: Box::new(convert(first, model)),
                    second: Box::new(convert(second, model)),
                },
            }
        }
        let workspace = self
            .organization
            .workspaces
            .iter()
            .find(|w| w.tabs.iter().any(|t| t.id == tab))?;
        let layout = self.layouts.get(tab)?;
        Some(actions::LayoutDescription {
            workspace_id: workspace.id.clone(),
            tab_id: tab.into(),
            zoomed: self.zoomed_tabs.contains(tab),
            focused_pane_id: self.records.get(&layout.focused())?.address.clone(),
            root: convert(layout.root(), self),
        })
    }
    pub fn layout_snapshot(&self, area: Rect) -> io::Result<actions::PaneLayoutSnapshot> {
        self.layout_snapshot_for(
            self.active_tab().ok_or_else(|| invalid("no active tab"))?,
            area,
        )
    }
    pub fn layout_snapshot_for(
        &self,
        tab: &str,
        area: Rect,
    ) -> io::Result<actions::PaneLayoutSnapshot> {
        let workspace = self
            .organization
            .workspaces
            .iter()
            .find(|w| w.tabs.iter().any(|t| t.id == tab))
            .ok_or_else(|| invalid("Core tab not found"))?;
        let layout = self.layouts.get(tab).ok_or_else(|| invalid("no panes"))?;
        let rect = actions::PaneLayoutRect::from;
        Ok(actions::PaneLayoutSnapshot {
            workspace_id: workspace.id.clone(),
            tab_id: tab.into(),
            zoomed: self.zoomed_tabs.contains(tab),
            area: rect(area),
            focused_pane_id: self.records[&layout.focused()].address.clone(),
            panes: layout
                .panes(area)
                .into_iter()
                .map(|p| actions::PaneLayoutPane {
                    pane_id: self.records[&p.id].address.clone(),
                    focused: p.is_focused,
                    rect: rect(p.rect),
                })
                .collect(),
            splits: layout
                .splits(area)
                .into_iter()
                .enumerate()
                .map(|(i, s)| actions::PaneLayoutSplit {
                    id: i.to_string(),
                    direction: if s.direction == Direction::Horizontal {
                        actions::SplitDirection::Right
                    } else {
                        actions::SplitDirection::Down
                    },
                    ratio: s.ratio,
                    rect: rect(s.area),
                })
                .collect(),
        })
    }
}
