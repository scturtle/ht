use super::*;

fn agent_snapshot(
    status: crate::actions::AgentStatus,
    sequence: u64,
    revision: u64,
) -> ClientShellSnapshot {
    let mut next = snapshot();
    next.revision = revision;
    let pane = &next.panes[0];
    next.agents = vec![crate::protocol::ClientShellAgent {
        pane_id: pane.pane_id.clone(),
        workspace_id: pane.workspace_id.clone(),
        tab_id: pane.tab_id.clone(),
        name: None,
        display_agent: None,
        agent: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: sequence,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    }];
    next
}

fn paint(revision: u64) -> PaneSurfaceFrame {
    let mut next = surface();
    next.projection_revision = revision;
    next.surface_revision = revision;
    next
}

#[test]
fn snapshot_install_preserves_the_incoming_allocation_for_presentation() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let incoming = Box::new(snapshot());
    let allocation = std::ptr::from_ref(incoming.as_ref());
    assert!(state.set_endpoint_snapshot_for_generation(7, incoming));
    assert!(
        std::ptr::eq(state.endpoint.snapshot.as_deref().unwrap(), allocation),
        "receiving a snapshot must move, not clone, its UI data"
    );
    assert!(state.endpoint_is_online());
    assert!(
        state.compose(100, 28).is_some(),
        "startup chrome remains available before a surface"
    );
}

#[test]
fn surface_acknowledgement_updates_the_presented_snapshot_in_place() {
    use crate::actions::AgentStatus::{Done, Idle, Working};
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.outer_focused = Some(false);
    state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Working, 1, 1)));
    state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Done, 2, 2)));
    let surface = paint(2);
    state.set_pane_surface(surface.clone());
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Done
    );
    let allocation = std::ptr::from_ref(state.endpoint.snapshot.as_deref().unwrap());
    state.outer_focused = Some(true);
    assert!(state.acknowledge_active_surface_agents(&surface, Some(7)));
    let accepted = state.endpoint.snapshot.as_deref().unwrap();
    assert!(std::ptr::eq(accepted, allocation));
    assert_eq!(accepted.agents[0].agent_status, Idle);
    assert_eq!(accepted.tabs[0].agent_status, Idle);
    assert_eq!(accepted.workspaces[0].agent_status, Idle);
    assert!(!state.acknowledge_active_surface_agents(&surface, Some(7)));
}

#[test]
fn a_replacement_generation_cannot_acknowledge_an_old_matching_surface() {
    use crate::actions::AgentStatus::{Done, Idle, Working};
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Working, 1, 1)));
    state.set_pane_surface(paint(1));
    assert!(state.compose(100, 28).is_some());
    // Same boot/revision, but a different delivery generation. The old pixels
    // cannot mark this new completion as seen or become the new generation's paint.
    assert!(state.set_endpoint_snapshot_for_generation(8, Box::new(agent_snapshot(Done, 2, 1))));
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Done
    );
    assert!(state.compose(100, 28).is_none());
    state.handle_raw_events(vec![
        RawInputEvent::OuterFocusLost,
        RawInputEvent::OuterFocusGained,
    ]);
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Done
    );
    state.set_pane_surface(paint(1));
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Idle
    );
    assert!(state.compose(100, 28).is_some());
}

#[test]
fn future_surfaces_only_acknowledge_their_exact_snapshot() {
    use crate::actions::AgentStatus::{Done, Idle, Working};
    for future in [3, 4] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Working, 1, 1)));
        state.set_pane_surface(paint(1));
        state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Done, 2, 2)));
        state.set_pane_surface(paint(future));
        assert_eq!(
            state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
            Done
        );
        assert!(state.compose(100, 28).is_none());
        state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Idle, 2, future)));
        assert_eq!(
            state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
            Idle
        );
        assert!(state.pending_pane_surface.is_none());
        assert!(state.compose(100, 28).is_some());
    }
}

#[test]
fn navigation_initialization_uses_the_new_snapshot_scope() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.mode = ClientShellMode::Navigate;
    state.set_endpoint_snapshot_for_generation(7, Box::new(snapshot()));
    assert!(state.navigation_target_valid(state.navigate_workspace_id.as_ref().unwrap()));
    state.navigate_workspace_id = None;
    let mut replacement = snapshot();
    replacement.boot_id = "replacement".into();
    state.set_endpoint_snapshot_for_generation(8, Box::new(replacement));
    let target = state.navigate_workspace_id.as_ref().unwrap();
    assert!(state.navigation_target_valid(target));
    assert_eq!(target.workspace_id, "ws_1");
}

#[test]
fn stale_snapshots_cannot_replace_the_allocation_or_change_completion_history() {
    use crate::actions::AgentStatus::{Done, Idle, Working};
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Working, 5, 5)));
    state.set_pane_surface(paint(5));
    let allocation = std::ptr::from_ref(state.endpoint.snapshot.as_deref().unwrap());
    let recency = state.endpoint.agent_recency.clone();
    assert!(!state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Idle, 99, 4))));
    assert!(std::ptr::eq(
        state.endpoint.snapshot.as_deref().unwrap(),
        allocation
    ));
    assert_eq!(state.endpoint.agent_recency, recency);
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Working
    );
    state.set_endpoint_snapshot_for_generation(7, Box::new(agent_snapshot(Done, 6, 6)));
    assert_eq!(
        state.endpoint.snapshot.as_ref().unwrap().agents[0].agent_status,
        Done
    );
}
