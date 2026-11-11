use super::*;
use crate::client::endpoint::{ClientEndpointId, ClientEndpointStatus};

#[test]
fn a_new_projection_epoch_accepts_restarted_revisions_but_its_stale_updates_do_not() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut old = snapshot();
    old.revision = 80;
    assert!(state.set_endpoint_snapshot_for_generation(&ClientEndpointId::Local, 1, Box::new(old)));
    let mut paint = surface();
    paint.projection_revision = 80;
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    let mut fresh = snapshot();
    fresh.boot_id = "replacement-projection".into();
    assert!(state.set_endpoint_snapshot_for_generation(
        &ClientEndpointId::Local,
        1,
        Box::new(fresh.clone())
    ));
    assert_eq!(state.snapshot.as_ref().unwrap().revision, 1);
    let mut paint = surface();
    paint.boot_id = fresh.boot_id.clone();
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    fresh.revision = 0;
    assert!(!state.set_endpoint_snapshot_for_generation(
        &ClientEndpointId::Local,
        1,
        Box::new(fresh)
    ));
    assert_eq!(state.snapshot.as_ref().unwrap().revision, 1);
}

#[test]
fn core_settings_and_context_menu_only_offer_implemented_actions() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_endpoint_methods(Some(vec![]));
    state.open_settings_overlay();
    let Some(ClientShellOverlay::Settings(settings)) = &state.overlay else {
        panic!("settings");
    };
    assert_eq!(
        settings.sections,
        vec![
            ClientSettingsSection::Theme,
            ClientSettingsSection::Indicators,
            ClientSettingsSection::Sound
        ]
    );
    let mut outcome = ClientShellInput::default();
    state.select_settings_section(ClientSettingsSection::Integrations, &mut outcome);
    assert!(outcome.actions.is_empty());
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ReloadConfig),
        &mut outcome,
    );
    assert!(
        outcome.actions.is_empty(),
        "Core config reload must remain client-local"
    );
    let menu = ClientContextMenuOverlay {
        methods: Some(Default::default()),
        target: ClientContextMenuTarget::Pane {
            pane_id: "p".into(),
            workspace_id: "w".into(),
            source_pane_id: None,
            has_manual_label: true,
            right_click_passthrough: false,
        },
        x: 0,
        y: 0,
        highlighted: 0,
    };
    assert!(menu.items().iter().all(|i| matches!(
        i.action,
        ClientContextMenuAction::ToggleRightClickPassthrough
            | ClientContextMenuAction::DumpPaneDiagnostics
    )));
}

fn pending_worktree() -> (ClientShellState, Vec<ClientShellAction>) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    submit_worktree(state)
}

fn submit_worktree(mut state: ClientShellState) -> (ClientShellState, Vec<ClientShellAction>) {
    let boot_id = state.snapshot.as_ref().unwrap().boot_id.clone();
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorktree),
        &mut outcome,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected worktree preparation");
    };
    state.handle_endpoint_result(&boot_id, &request.id, Ok(worktree_list_result(None)));
    state.handle_input_bytes(b"feature/reconnect");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::WorktreeCreate(create)) if create.creating
    ));
    (state, outcome.actions)
}

fn request_id(actions: &[ClientShellAction]) -> &str {
    let [ClientShellAction::Endpoint { request, .. }] = actions else {
        panic!("expected one endpoint request");
    };
    &request.id
}

fn worktree_created_result() -> crate::api::schema::ResponseResult {
    serde_json::from_value(serde_json::json!({
        "type": "worktree_created",
        "workspace": {
            "workspace_id": "ws_2", "number": 2, "label": "worktree",
            "focused": false, "pane_count": 1, "tab_count": 1,
            "active_tab_id": "tab_2", "agent_status": "idle"
        },
        "tab": {
            "tab_id": "tab_2", "workspace_id": "ws_2", "number": 1,
            "label": "worktree", "focused": false, "pane_count": 1,
            "agent_status": "idle"
        },
        "root_pane": {
            "pane_id": "pane_2", "terminal_id": "term_2", "workspace_id": "ws_2",
            "tab_id": "tab_2", "focused": false, "agent_status": "idle", "revision": 0
        },
        "worktree": {
            "path": "/repo-feature", "branch": "feature/reconnect", "is_bare": false,
            "is_detached": false, "is_prunable": false, "is_linked_worktree": true,
            "open_workspace_id": "ws_2", "label": "worktree"
        }
    }))
    .unwrap()
}

#[test]
fn worktree_create_leaves_server_focus_unchanged() {
    let (_, actions) = pending_worktree();
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected worktree creation");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorktreeCreate(params) if !params.focus
    ));
}

#[test]
fn worktree_create_success_focuses_returned_tab_on_its_endpoint_after_snapshot_update() {
    {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot()));
        let (mut state, actions) = submit_worktree(state);
        let endpoint_id = state.active_endpoint_id.clone();
        let mut updated = state.snapshot.clone().unwrap();
        let boot_id = updated.boot_id.clone();
        updated.revision += 1;
        updated.workspaces[0].label = "updated while creating".into();
        state.set_endpoint_snapshot(&endpoint_id, updated);
        let (repaint, focus) = state.handle_endpoint_result(
            &boot_id,
            request_id(&actions),
            Ok(worktree_created_result()),
        );
        assert!(repaint);
        assert!(state.overlay.is_none());
        let [ClientShellAction::Endpoint {
            endpoint_id: target,
            boot_id: target_boot,
            request,
        }] = &focus[..]
        else {
            panic!("creation should request focus through normal client navigation");
        };
        assert_eq!(target, &endpoint_id);
        assert_eq!(target_boot, &boot_id);
        assert!(matches!(
            &request.method,
            crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2"
        ));
        assert!(state
            .handle_endpoint_result(
                &boot_id,
                request_id(&actions),
                Ok(worktree_created_result())
            )
            .1
            .is_empty());
    }
}

#[test]
fn worktree_create_cancelled_or_failed_request_never_focuses() {
    for cancel in [false, true] {
        let (mut state, actions) = pending_worktree();
        let id = request_id(&actions);
        if cancel {
            assert!(state.cancel_endpoint_request(id));
        } else {
            let (_, follow_up) = state.handle_endpoint_result(
                "boot-1",
                id,
                Err(ClientShellEndpointError {
                    code: Some("worktree_failed".into()),
                    message: "creation failed".into(),
                }),
            );
            assert!(follow_up.is_empty());
        }
        assert!(state
            .handle_endpoint_result("boot-1", id, Ok(worktree_created_result()))
            .1
            .is_empty());
    }
}

#[test]
fn disconnect_cancels_worktree_dialog_before_same_server_reconnect() {
    let (mut state, _) = pending_worktree();
    state.mark_endpoint_disconnected(&ClientEndpointId::Local);
    assert!(state.pending_requests.is_empty());
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::WorktreeCreate(create)) if !create.creating
    ));
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    state.set_endpoint_snapshot(&ClientEndpointId::Local, Box::new(snapshot()));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.set_pane_surface(surface());
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    ))]);
    assert!(state.overlay.is_none());
    assert!(!state.handle_input_bytes(b"x").requests.is_empty());
}

struct TestTransport {
    fail: bool,
}

impl crate::client::endpoint::EndpointTransport for TestTransport {
    fn send(&mut self, _: &ClientMessage) -> std::io::Result<()> {
        if self.fail {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn local_selection_does_not_need_an_endpoint_activation_event() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    tx.try_send(crate::client::ClientLoopEvent::Timer).unwrap();
    let mut outcome = ClientShellInput::default();
    assert!(state.focus_or_activate(
        ClientEndpointId::Local,
        ClientEndpointFocusTarget::Workspace("ws_1".into()),
        &mut outcome
    ));
    assert!(
        matches!(&outcome.actions[..],[ClientShellAction::Endpoint {request,..}] if matches!(request.method,crate::api::schema::Method::WorkspaceFocus(_)))
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(crate::client::ClientLoopEvent::Timer)
    ));
}

#[test]
fn dispatcher_cancels_requests_when_the_core_transport_fails() {
    use crate::client::endpoint::EndpointRegistry;
    use crate::client::endpoint_commands::EndpointCommands;

    {
        let fail_send = true;
        let (mut state, actions) = pending_worktree();
        let mut endpoints = EndpointRegistry::new(TestTransport { fail: fail_send }, 1);
        let mut commands = EndpointCommands::default();
        let (replay, repaint) = crate::client::shell_runtime::dispatch_client_shell_actions(
            actions,
            &mut commands,
            &mut endpoints,
            &mut state,
            &mut Vec::new(),
        )
        .unwrap();
        assert!(repaint);
        assert!(replay.is_empty());
        assert!(state.pending_requests.is_empty());
        assert!(matches!(
            &state.overlay,
            Some(ClientShellOverlay::WorktreeCreate(create)) if !create.creating
        ));
        assert!(state
            .visible_endpoint_notice
            .as_ref()
            .is_some_and(|notice| { notice.title == "Action interrupted" }));
        assert_eq!(commands.diagnostic_snapshot()["queued"], 0);
    }
}

#[test]
fn stale_queued_request_is_cancelled_without_blocking_the_current_generation() {
    use crate::client::endpoint::EndpointRegistry;
    use crate::client::endpoint_commands::EndpointCommands;

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let actions =
        state.focus_endpoint_target(ClientEndpointFocusTarget::Workspace("ws_stale".into()));
    let stale_id = request_id(&actions).to_owned();
    let current = state.focus_endpoint_target(ClientEndpointFocusTarget::Workspace("ws_1".into()));
    let current_id = request_id(&current).to_owned();
    let mut commands = EndpointCommands::default();
    for (generation, actions) in [(1, actions), (2, current)] {
        for action in actions {
            let ClientShellAction::Endpoint {
                endpoint_id,
                boot_id,
                request,
            } = action
            else {
                panic!("expected endpoint request");
            };
            commands.enqueue(endpoint_id, generation, boot_id, request);
        }
    }
    let mut endpoints = EndpointRegistry::new(TestTransport { fail: false }, 2);
    let cancelled = commands.send_next(&ClientEndpointId::Local, &mut endpoints);
    assert_eq!(cancelled, vec![stale_id.clone()]);
    state.cancel_endpoint_request(&stale_id);
    assert!(!commands.accepts_response(&ClientEndpointId::Local, 1, "boot-1", &stale_id));
    assert!(commands.accepts_response(&ClientEndpointId::Local, 2, "boot-1", &current_id));
    assert!(state.pending_requests.contains_key(&current_id));
}

#[test]
fn cancelled_integration_install_does_not_queue_a_refresh() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.open_settings_overlay();
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_mut() else {
        panic!("settings overlay");
    };
    settings.installing_integrations = true;
    state.pending_integration_installs = 1;
    let mut outcome = ClientShellInput::default();
    assert!(state.push_endpoint_method_with_kind(
        crate::api::schema::Method::IntegrationInstall(
            crate::api::schema::IntegrationInstallParams {
                target: crate::api::schema::IntegrationTarget::Pi,
            }
        ),
        PendingEndpointKind::IntegrationInstall,
        &mut outcome,
    ));
    assert!(state.cancel_endpoint_request(request_id(&outcome.actions)));
    assert!(state.pending_requests.is_empty());
    assert_eq!(state.pending_integration_installs, 0);
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::Settings(settings))
            if !settings.installing_integrations && !settings.loading_integrations
    ));
}

#[test]
fn failed_selection_copy_does_not_send_terminal_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.selection = Some(crate::selection::Selection::absolute_range(
        "pane_1".into(),
        (0, 0),
        (0, 2),
    ));
    for result in [
        Ok(crate::api::schema::ResponseResult::PaneSelection {
            pane_id: "pane_1".into(),
            text: String::new(),
        }),
        Err(ClientShellEndpointError {
            code: Some("endpoint_cancelled".into()),
            message: "cancelled".into(),
        }),
        Err(ClientShellEndpointError {
            code: Some("selection_unavailable".into()),
            message: "selection text is unavailable".into(),
        }),
    ] {
        let mut outcome = ClientShellInput::default();
        state.request_selection_copy(&mut outcome, false);
        let (_, actions) =
            state.handle_endpoint_result("boot-1", request_id(&outcome.actions), result);
        assert!(actions.is_empty());
        assert!(state.pending_requests.is_empty());
    }
}

#[test]
fn cancelled_link_activation_does_not_replay_mouse_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    let pane_id = state.hits.panes[0].pane_id.clone();
    let inner_rect = state.hits.panes[0].inner_rect;
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method_with_kind(
        crate::api::schema::Method::PaneLinkActivate(crate::api::schema::PaneLinkActivateParams {
            pane_id: pane_id.clone(),
            viewport_row: 0,
            col: 0,
            content_revision: None,
            offset_from_bottom: None,
        }),
        PendingEndpointKind::PaneLinkActivate {
            pane_id,
            inner_rect,
            fallback_events: vec![MouseEvent {
                kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: inner_rect.x,
                row: inner_rect.y,
                modifiers: KeyModifiers::NONE,
            }],
        },
        &mut outcome,
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        request_id(&outcome.actions),
        Err(ClientShellEndpointError {
            code: Some("endpoint_cancelled".into()),
            message: "cancelled".into(),
        }),
    );
    assert!(actions.is_empty());
    assert!(state.url_click_consumes_until_up);
}
