use super::*;

fn workspaces(count: usize) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces = (1..=count)
        .map(|number| {
            let mut workspace = projected.workspaces[0].clone();
            workspace.workspace_id = format!("ws_{number}");
            workspace.number = number;
            workspace.focused = number == 1;
            workspace
        })
        .collect();
    projected
}

fn grouped_workspaces() -> ClientShellSnapshot {
    let mut projected = workspaces(3);
    for (index, linked) in [(0, false), (2, true)] {
        projected.workspaces[index].worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: linked,
        });
    }
    projected
}

fn preview_key(state: &mut ClientShellState, bytes: &[u8]) {
    let outcome = state.handle_input_bytes(bytes);
    assert!(outcome.actions.is_empty(), "{bytes:?}");
    assert!(outcome.requests.is_empty(), "{bytes:?}");
    assert!(outcome.repaint, "{bytes:?}");
}

fn enter_navigation(state: &mut ClientShellState) {
    preview_key(state, &[0x02]);
    preview_key(state, b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

fn assert_selected(state: &ClientShellState, workspace: &str) {
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(workspace)
    );
}

#[test]
fn empty_workspace_navigation_enter_exits_without_focusing() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut empty = workspaces(0);
    empty.tabs.clear();
    empty.panes.clear();
    empty.focused_workspace_id = None;
    empty.focused_tab_id = None;
    empty.focused_pane_id = None;
    state.set_snapshot(Box::new(empty));
    enter_navigation(&mut state);
    assert!(state.navigate_workspace_id.is_none());
    let enter = state.handle_input_bytes(b"\r");
    assert!(enter.actions.is_empty() && enter.requests.is_empty() && enter.repaint);
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn workspace_navigation_preserves_parent_child_group_order() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(grouped_workspaces()));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    enter_navigation(&mut state);
    for id in ["ws_3", "ws_2"] {
        preview_key(&mut state, b"\x1b[B");
        assert_selected(&state, id);
    }
}

#[test]
fn active_preview_is_not_retargeted_by_deletion_or_reboot() {
    for invalidation in ["deleted", "boot", "generation"] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut local = workspaces(2);
        state.set_endpoint_snapshot_for_generation(7, Box::new(local.clone()));
        state.set_pane_surface(surface());
        state.compose(100, 28).unwrap();
        enter_navigation(&mut state);
        preview_key(&mut state, b"\x1b[B");
        assert_selected(&state, "ws_2");
        let selected = state.navigate_workspace_id.clone();
        match invalidation {
            "boot" => local.boot_id = "new-local-boot".into(),
            "deleted" => {
                local.revision += 1;
                local.workspaces.pop();
            }
            _ => {}
        }
        let generation = if invalidation == "generation" { 8 } else { 7 };
        state.set_endpoint_snapshot_for_generation(generation, Box::new(local));
        assert_eq!(state.navigate_workspace_id, selected);
        preview_key(&mut state, b"\r");
        assert_eq!(state.mode, ClientShellMode::Navigate);
        assert!(state.visible_endpoint_notice.is_some());
        for confirm in [false, true] {
            state.config.confirm_close = confirm;
            for key in [b"W", b"D"] {
                preview_key(&mut state, key);
            }
            assert!(state.overlay.is_none());
            assert_eq!(state.mode, ClientShellMode::Navigate);
        }
        assert_eq!(state.workspace_action_id().as_deref(), Some("ws_1"));
    }
}

fn local_navigation_state(compact: bool) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.palette = Palette::terminal();
    state.config.mobile_width_threshold = if compact { u16::MAX } else { 0 };
    state.set_snapshot(Box::new(workspaces(3)));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    state
}

fn request_local_navigation(state: &mut ClientShellState, down: usize) -> String {
    enter_navigation(state);
    for _ in 0..down {
        preview_key(state, b"\x1b[B");
    }
    let outcome = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
        panic!("expected a local workspace focus request");
    };
    assert!(matches!(
        request.action,
        crate::actions::Action::WorkspaceFocus(_)
    ));
    request.id.clone()
}

fn assert_local_highlight(state: &mut ClientShellState, selected_id: &str) {
    state.compose(100, 28).expect("frame");
    let target = state
        .pending_workspace_highlight
        .as_ref()
        .map(|p| p.target.workspace_id.as_str())
        .or_else(|| {
            state
                .endpoint
                .snapshot
                .as_deref()
                .and_then(|s| s.focused_workspace_id.as_deref())
        });
    assert_eq!(target, Some(selected_id));
}

fn set_local_focus(state: &mut ClientShellState, workspace_id: &str, revision: u64) {
    let mut snapshot = workspaces(3);
    snapshot.revision = revision;
    snapshot.focused_workspace_id = Some(workspace_id.into());
    for workspace in &mut snapshot.workspaces {
        workspace.focused = workspace.workspace_id == workspace_id;
    }
    state.set_snapshot(Box::new(snapshot));
    let mut frame = surface();
    frame.projection_revision = revision;
    state.set_pane_surface(frame);
}

#[test]
fn accepted_local_navigation_keeps_highlight_until_authoritative_focus() {
    for compact in [false, true] {
        for response_first in [false, true] {
            let mut state = local_navigation_state(compact);
            let request_id = request_local_navigation(&mut state, 2);
            assert_eq!(state.mode, ClientShellMode::Terminal);
            assert!(state.navigate_workspace_id.is_none());
            assert_eq!(
                state
                    .endpoint
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .focused_workspace_id
                    .as_deref(),
                Some("ws_1")
            );
            assert_eq!(state.focused_pane_id().as_deref(), Some("pane_1"));
            assert_local_highlight(&mut state, "ws_3");
            state.invalidate_pane_surface();
            assert_local_highlight(&mut state, "ws_3");
            state.set_pane_surface(surface());
            if response_first {
                state.handle_endpoint_result(
                    "boot-1",
                    &request_id,
                    Ok(crate::actions::ActionResult::Ok {}),
                );
                assert_local_highlight(&mut state, "ws_3");
            }
            set_local_focus(&mut state, "ws_1", 2);
            assert_local_highlight(&mut state, "ws_3");
            set_local_focus(&mut state, "ws_3", 3);
            assert_local_highlight(&mut state, "ws_3");
            if !response_first {
                state.handle_endpoint_result(
                    "boot-1",
                    &request_id,
                    Ok(crate::actions::ActionResult::Ok {}),
                );
            }
            set_local_focus(&mut state, "ws_2", 4);
            assert_local_highlight(&mut state, "ws_2");
        }
    }
}

#[test]
fn failed_local_navigation_releases_only_its_own_highlight() {
    for failure in ["rejected", "endpoint_timeout", "cancelled"] {
        let mut state = local_navigation_state(false);
        let request_id = request_local_navigation(&mut state, 2);
        assert_local_highlight(&mut state, "ws_3");
        if failure == "cancelled" {
            assert!(state.cancel_endpoint_request(&request_id));
        } else {
            state.handle_endpoint_result(
                "boot-1",
                &request_id,
                Err(ClientShellEndpointError {
                    code: Some(failure.into()),
                    message: "focus failed".into(),
                }),
            );
        }
        assert_local_highlight(&mut state, "ws_1");
        state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Ok(crate::actions::ActionResult::Ok {}),
        );
        assert_local_highlight(&mut state, "ws_1");
    }
    for old_down in [1, 2] {
        let mut state = local_navigation_state(false);
        let old_request = request_local_navigation(&mut state, old_down);
        let latest_request = request_local_navigation(&mut state, 2);
        state.cancel_endpoint_request(&old_request);
        assert_local_highlight(&mut state, "ws_3");
        state.cancel_endpoint_request(&latest_request);
        assert_local_highlight(&mut state, "ws_1");
    }
}

#[test]
fn pending_navigation_highlight_does_not_survive_identity_changes() {
    for change in ["disconnect", "boot", "generation", "deleted"] {
        let mut state = local_navigation_state(false);
        let request_id = request_local_navigation(&mut state, 2);
        state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Ok(crate::actions::ActionResult::Ok {}),
        );
        assert_local_highlight(&mut state, "ws_3");
        let mut snapshot = workspaces(3);
        match change {
            "disconnect" => {
                state.mark_endpoint_disconnected();
                state.set_endpoint_status(ClientEndpointStatus::Online);
                state.set_snapshot(Box::new(snapshot));
            }
            "boot" => {
                snapshot.boot_id = "replacement-boot".into();
                state.set_snapshot(Box::new(snapshot));
            }
            "generation" => {
                state.set_endpoint_snapshot_for_generation(1, Box::new(snapshot));
            }
            "deleted" => {
                snapshot.workspaces.pop();
                state.set_snapshot(Box::new(snapshot));
                state.set_snapshot(Box::new(workspaces(3)));
            }
            _ => unreachable!(),
        }
        assert!(state.pending_workspace_highlight.is_none(), "{change}");
        let mut frame = surface();
        frame.boot_id = state.endpoint.snapshot.as_ref().unwrap().boot_id.clone();
        state.set_pane_surface(frame);
        assert_local_highlight(&mut state, "ws_1");
    }
}

#[test]
fn navigation_highlight_requires_enqueued_focus_and_yields_to_new_intent() {
    let mut state = local_navigation_state(false);
    state.set_endpoint_methods(Some(Vec::new()));
    enter_navigation(&mut state);
    preview_key(&mut state, b"\x1b[B");
    let refused = state.handle_input_bytes(b"\r");
    assert!(refused.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.pending_workspace_highlight.is_none());
    // The unsupported-action notice spans the sidebar on this narrow frame.
    assert!(state.visible_endpoint_notice.take().is_some());
    assert_local_highlight(&mut state, "ws_1");

    state.set_endpoint_methods(None);
    request_local_navigation(&mut state, 2);
    enter_navigation(&mut state);
    preview_key(&mut state, b"\x1b");
    assert_local_highlight(&mut state, "ws_1");

    request_local_navigation(&mut state, 2);
    let mut unrelated = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::PaneScroll(crate::actions::PaneScrollParams {
            pane_id: state.endpoint.snapshot.as_ref().unwrap().panes[0]
                .pane_id
                .clone(),
            offset_from_bottom: 0,
        }),
        &mut unrelated,
    );
    let [ClientShellAction::Endpoint { request, .. }] = unrelated.actions.as_slice() else {
        panic!("expected unrelated request");
    };
    state.cancel_endpoint_request(&request.id);
    assert_local_highlight(&mut state, "ws_3");
    let mut focus = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
            workspace_id: "ws_2".into(),
        }),
        &mut focus,
    );
    assert!(state.pending_workspace_highlight.is_none());
}

#[test]
fn directional_pane_focus_releases_an_accepted_workspace_highlight() {
    use crate::actions::{Action, ActionResult, PaneDirection};

    for (key, direction) in [
        (b'h', PaneDirection::Left),
        (b'j', PaneDirection::Down),
        (b'k', PaneDirection::Up),
        (b'l', PaneDirection::Right),
    ] {
        for rejected in [false, true] {
            let mut state = local_navigation_state(false);
            let pending_request = request_local_navigation(&mut state, 2);
            assert_local_highlight(&mut state, "ws_3");
            preview_key(&mut state, &[0x02]);
            let outcome = state.handle_input_bytes(&[key]);
            let [ClientShellAction::Endpoint { request, .. }] = outcome.actions.as_slice() else {
                panic!("expected a directional pane focus request");
            };
            let Action::PaneFocusDirection(params) = &request.action else {
                panic!("expected PaneFocusDirection");
            };
            assert_eq!(params.direction, direction);
            assert_eq!(params.pane_id.as_deref(), Some("pane_1"));
            assert!(state.pending_workspace_highlight.is_none());
            assert_local_highlight(&mut state, "ws_1");
            let result = if rejected {
                Err(ClientShellEndpointError {
                    code: Some("rejected".into()),
                    message: "focus rejected".into(),
                })
            } else {
                Ok(ActionResult::Ok {})
            };
            state.handle_endpoint_result("boot-1", &pending_request, result);
            assert_local_highlight(&mut state, "ws_1");
        }
    }
}

#[test]
fn direct_agent_focus_repaints_when_releasing_a_workspace_highlight() {
    let mut config = Config::default();
    config.keys.focus_agent = crate::config::BindingConfig::one("ctrl+alt+1");
    let mut projected = workspaces(3);
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });

    for pending in [false, true] {
        let mut state = local_navigation_state(false);
        state.config.keybinds = ClientShellConfig::from_config(&config).keybinds;
        state.set_snapshot(Box::new(projected.clone()));
        state.compose(100, 28).unwrap();
        if pending {
            request_local_navigation(&mut state, 2);
            assert_local_highlight(&mut state, "ws_3");
        }

        // Direct bindings do not inherit the repaint from leaving prefix mode.
        let outcome =
            state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
                KeyCode::Char('1'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ))]);
        assert!(
            matches!(outcome.actions.as_slice(), [ClientShellAction::Endpoint { request, .. }]
            if matches!(&request.action, crate::actions::Action::PaneFocus(params)
                if params.pane_id == "pane_1"))
        );
        assert!(state.pending_workspace_highlight.is_none());
        assert_eq!(outcome.repaint, pending);
        assert_local_highlight(&mut state, "ws_1");
    }
}

#[test]
fn cancelled_close_does_not_restore_an_older_navigation_highlight() {
    let mut state = local_navigation_state(false);
    request_local_navigation(&mut state, 2);
    state.open_confirm_close_overlay("ws_1".into());
    preview_key(&mut state, b"\x1b");
    assert_eq!(state.mode, ClientShellMode::Navigate);
    preview_key(&mut state, b"\x1b");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert_local_highlight(&mut state, "ws_1");
}

#[test]
fn coalesced_navigation_focus_does_not_leave_a_permanent_highlight() {
    let mut state = local_navigation_state(false);
    let before_request = std::time::Instant::now();
    let request_id = request_local_navigation(&mut state, 2);
    state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::actions::ActionResult::Ok {}),
    );
    // Another client can focus the original workspace before the server projects
    // either change, so a successful request need not produce a new snapshot.
    assert!(!state.tick_workspace_highlight(before_request));
    assert_local_highlight(&mut state, "ws_3");
    let now = std::time::Instant::now();
    assert!(state.tick_workspace_highlight(now + std::time::Duration::from_secs(2)));
    assert_local_highlight(&mut state, "ws_1");
    assert!(!state.tick_workspace_highlight(now + std::time::Duration::from_secs(3)));
}

#[test]
fn navigation_highlight_ends_for_noop_focus_and_focused_creation() {
    let mut state = local_navigation_state(false);
    request_local_navigation(&mut state, 0);
    assert!(state.pending_workspace_highlight.is_none());
    set_local_focus(&mut state, "ws_2", 2);
    assert_local_highlight(&mut state, "ws_2");

    for focus in [false, true] {
        for method in [
            crate::actions::Action::WorkspaceCreate(crate::actions::WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: None,
                focus,
                label: None,
                env: Default::default(),
            }),
            crate::actions::Action::TabCreate(crate::actions::TabCreateParams {
                workspace_id: Some("ws_1".into()),
                cwd: None,
                focus,
                label: None,
                env: Default::default(),
            }),
        ] {
            let mut state = local_navigation_state(false);
            request_local_navigation(&mut state, 2);
            let mut outcome = ClientShellInput::default();
            state.push_endpoint_method(method, &mut outcome);
            assert_eq!(state.pending_workspace_highlight.is_none(), focus);
            assert_local_highlight(&mut state, if focus { "ws_1" } else { "ws_3" });
        }
    }
}

// Kitty keyboard protocol (CSI u) encodings of `[` (codepoint 91).
const KITTY_CTRL_BRACKET: &[u8] = b"\x1b[91;5u";
const KITTY_CTRL_SHIFT_BRACKET: &[u8] = b"\x1b[91;6u";

#[test]
fn kitty_ctrl_bracket_leaves_navigation_like_esc() {
    let mut state = local_navigation_state(false);
    enter_navigation(&mut state);
    preview_key(&mut state, b"\x1b[B");

    preview_key(&mut state, KITTY_CTRL_BRACKET);

    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.navigate_workspace_id.is_none());
}

#[test]
fn configured_ctrl_bracket_navigate_binding_wins_over_cancel() {
    let config: Config = toml::from_str("[keys]\nnavigate_workspace_down = \"ctrl+[\"\n").unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(workspaces(3)));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    enter_navigation(&mut state);
    assert_selected(&state, "ws_1");

    preview_key(&mut state, KITTY_CTRL_BRACKET);

    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_selected(&state, "ws_2");
}

#[test]
fn kitty_ctrl_shift_bracket_keeps_navigation_open() {
    let mut state = local_navigation_state(false);
    enter_navigation(&mut state);

    state.handle_input_bytes(KITTY_CTRL_SHIFT_BRACKET);

    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_selected(&state, "ws_1");
}
