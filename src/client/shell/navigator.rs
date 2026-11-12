//! Searchable workspace/pane rows for the client navigator.

use super::*;

pub(super) fn navigator_rows(
    endpoint: &ClientShellEndpoint,
    navigator: &ClientNavigatorOverlay,
) -> Vec<ClientNavigatorRow> {
    let query = navigator.query.trim().to_lowercase();
    let filter = |status| match navigator.filter {
        Some(ClientNavigatorFilter::Blocked) => status == crate::actions::AgentStatus::Blocked,
        Some(ClientNavigatorFilter::Working) => status == crate::actions::AgentStatus::Working,
        Some(ClientNavigatorFilter::Idle) => status == crate::actions::AgentStatus::Idle,
        Some(ClientNavigatorFilter::Done) => status == crate::actions::AgentStatus::Done,
        None => true,
    };
    let words = query.split_whitespace().collect::<Vec<_>>();
    let text = |value: &str| {
        if words.is_empty() {
            return true;
        }
        let value = value.to_lowercase();
        words.iter().all(|word| value.contains(word))
    };
    let filtering = navigator.filter.is_some() || !query.is_empty();
    let mut rows = Vec::new();

    let stale = endpoint.status != ClientEndpointStatus::Online;
    let endpoint_query_matches = !query.is_empty() && text(&endpoint.label);
    if let Some(snapshot) = endpoint.snapshot.as_deref() {
        let agents = snapshot
            .agents
            .iter()
            .map(|agent| (agent.pane_id.as_str(), agent))
            .collect::<HashMap<_, _>>();
        // Build indexes once and preserve snapshot order within each bucket.
        let mut tabs_by_workspace = HashMap::new();
        for tab in &snapshot.tabs {
            tabs_by_workspace
                .entry(tab.workspace_id.as_str())
                .or_insert_with(Vec::new)
                .push(tab);
        }
        let mut panes_by_tab = HashMap::new();
        for pane in &snapshot.panes {
            panes_by_tab
                .entry(pane.tab_id.as_str())
                .or_insert_with(Vec::new)
                .push(pane);
        }
        for workspace in &snapshot.workspaces {
            let workspace_matches = endpoint_query_matches
                || text(&workspace.label)
                || workspace.branch.as_deref().is_some_and(text);
            let mut children = Vec::new();
            let workspace_tabs = tabs_by_workspace
                .get(workspace.workspace_id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let multiple_tabs = workspace_tabs.len() > 1;
            for tab in workspace_tabs {
                let tab_matches = workspace_matches || text(&tab.label);
                let tab_panes = panes_by_tab
                    .get(tab.tab_id.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                for (index, pane) in tab_panes.iter().enumerate() {
                    let agent = agents.get(pane.pane_id.as_str()).copied();
                    let status = agent.map_or(crate::actions::AgentStatus::Unknown, |agent| {
                        agent.agent_status
                    });
                    let agent_kind = agent.and_then(|agent| {
                        agent.agent.as_deref().or(agent.display_agent.as_deref())
                    });
                    let name = pane
                        .label
                        .as_deref()
                        .or_else(|| agent.and_then(|agent| agent.name.as_deref()));
                    let title = agent.and_then(|agent| {
                        agent
                            .title
                            .as_deref()
                            .or(agent.terminal_title_stripped.as_deref())
                    });
                    let tab_name = (tab.custom_label || tab.label.parse::<usize>().is_err())
                        .then_some(tab.label.as_str());
                    let label = if tab_panes.len() == 1 {
                        match name.or(tab_name).or(title) {
                            Some(label) => label.to_owned(),
                            None if multiple_tabs => {
                                format!("{} · {}", agent_kind.unwrap_or("terminal"), tab.label)
                            }
                            None => workspace.label.clone(),
                        }
                    } else {
                        let pane_name = name.or(title).or(agent_kind).unwrap_or("terminal");
                        match tab_name {
                            Some(tab_name) if tab_name != pane_name => {
                                format!("{tab_name} · {pane_name} · {}", index + 1)
                            }
                            _ => format!("{pane_name} · {}", index + 1),
                        }
                    };
                    let meta = pane
                        .foreground_cwd
                        .as_deref()
                        .or(pane.cwd.as_deref())
                        .unwrap_or_default();
                    if filter(status)
                        && (tab_matches
                            || text(&label)
                            || text(meta)
                            || pane.cwd.as_deref().is_some_and(text)
                            || agent_kind.is_some_and(text)
                            || title.is_some_and(text)
                            || agent
                                .and_then(|agent| agent.display_agent.as_deref())
                                .is_some_and(text)
                            || text(&pane.pane_id))
                    {
                        children.push(ClientNavigatorRow {
                            depth: 1,
                            label,
                            meta: meta.to_owned(),
                            detail: format!(
                                "{} / {} / {}",
                                workspace.label, tab.label, pane.pane_id
                            ),
                            agent: agent_kind.map(str::to_owned),
                            status: Some(status),
                            stale,
                            current: snapshot.focused_pane_id.as_deref() == Some(&pane.pane_id),
                            target: ClientNavigatorTarget::Pane {
                                pane_id: pane.pane_id.clone(),
                            },
                        });
                    }
                }
            }
            if !filtering
                || !children.is_empty()
                || (navigator.filter.is_none() && !query.is_empty() && workspace_matches)
            {
                rows.push(ClientNavigatorRow {
                    depth: 0,
                    label: workspace.label.clone(),
                    meta: workspace.branch.clone().unwrap_or_default(),
                    detail: workspace.new_workspace_cwd.clone(),
                    agent: None,
                    status: None,
                    stale,
                    current: false,
                    target: ClientNavigatorTarget::Workspace {
                        workspace_id: workspace.workspace_id.clone(),
                    },
                });
                rows.extend(children);
            }
        }
    }

    rows
}

pub(super) fn navigator_selected_index(
    rows: &[ClientNavigatorRow],
    navigator: &ClientNavigatorOverlay,
) -> Option<usize> {
    match navigator.selected.as_ref() {
        Some(target) => rows.iter().position(|row| row.target == *target),
        None => rows
            .iter()
            .position(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. }))
            .or_else(|| (!rows.is_empty()).then_some(0)),
    }
}

pub(super) fn selected_navigator_target(
    rows: &[ClientNavigatorRow],
    navigator: &ClientNavigatorOverlay,
) -> Option<ClientNavigatorTarget> {
    navigator_selected_index(rows, navigator).map(|index| rows[index].target.clone())
}
