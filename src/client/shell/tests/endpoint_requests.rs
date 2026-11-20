use super::*;

#[test]
fn a_new_projection_epoch_accepts_restarted_revisions_but_its_stale_updates_do_not() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut old = snapshot();
    old.revision = 80;
    assert!(state.set_snapshot(Box::new(old)));
    let mut paint = surface();
    paint.projection_revision = 80;
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    let mut fresh = snapshot();
    fresh.boot_id = "replacement-projection".into();
    assert!(state.set_snapshot(Box::new(fresh.clone())));
    assert_eq!(state.endpoint.snapshot.as_ref().unwrap().revision, 1);
    let mut paint = surface();
    paint.boot_id = fresh.boot_id.clone();
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    fresh.revision = 0;
    assert!(!state.set_snapshot(Box::new(fresh)));
    assert_eq!(state.endpoint.snapshot.as_ref().unwrap().revision, 1);
}

#[test]
fn core_config_reload_and_context_menu_only_offer_implemented_actions() {
    let mut state = state_with_snapshot(snapshot());
    state.set_endpoint_methods(Some(vec![]));
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        keybindings::KeybindMatch::Action(keybindings::KeybindAction::ReloadConfig),
        &mut outcome,
    );
    assert!(outcome.refresh_host_colors);
    assert!(
        outcome.actions.is_empty(),
        "Core config reload must remain client-local"
    );
    let menu = ClientContextMenuOverlay {
        methods: Some(Default::default()),
        target: ClientContextMenuTarget::Pane {
            pane_id: "p".into(),
            workspace_id: "w".into(),
            tab_group: None,
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
    )));
}

fn request_id(actions: &[ClientShellAction]) -> &str {
    let [ClientShellAction::Endpoint { request, .. }] = actions else {
        panic!("expected one endpoint request");
    };
    &request.id
}

#[test]
fn capability_refresh_preserves_pending_request_boot_fences() {
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    let boot_id = state.endpoint.snapshot.as_ref().unwrap().boot_id.clone();
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::PaneFocus(crate::actions::PaneTarget {
            pane_id: "pane_1".into(),
        }),
        &mut outcome,
    );
    let pending_id = request_id(&outcome.actions).to_owned();

    state.set_endpoint_methods(Some(vec!["workspace.list".into()]));

    let pending = state.pending_requests.get(&pending_id).expect("fence kept");
    assert_eq!(pending.boot_id, boot_id);
    assert_eq!(pending.method_name, "pane.focus");

    // The fence still completes the request it was issued against.
    let (repaint, actions) = state.handle_endpoint_result(
        &boot_id,
        &pending_id,
        Ok(crate::actions::ActionResult::Ok {}),
    );
    assert!(!repaint && actions.is_empty());
    assert!(!state.pending_requests.contains_key(&pending_id));
}

#[test]
fn capability_refresh_regates_an_open_pane_context_menu() {
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.rename".into(), "pane.close".into()]));
    state.open_pane_context_menu("pane_1".into(), 0, 0);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!("pane context menu");
    };
    let close = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::ClosePane)
        .expect("close pane item");
    menu.highlighted = close;
    assert!(menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::RenamePane));
    state.compose(106, 20).expect("pane context menu rows");
    assert!(!state.hits.context_menu_rows.is_empty());

    // The replacement Core lost the optional pane.rename ability that Core only
    // advertises while it reports terminal.label support.
    assert!(state.set_endpoint_methods(Some(vec!["pane.close".into(),])));

    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("the open context menu survives a capability refresh");
    };
    let actions = menu
        .items()
        .iter()
        .map(|item| item.action)
        .collect::<Vec<_>>();
    assert!(
        !actions.contains(&ClientContextMenuAction::RenamePane),
        "{actions:?}"
    );
    assert_eq!(
        actions[menu.highlighted],
        ClientContextMenuAction::ClosePane
    );
    assert!(
        state.hits.context_menu_rows.is_empty(),
        "rows laid out for the old item list must not stay clickable"
    );

    let highlighted = menu.highlighted;
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(highlighted, &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("close request: {:?}", outcome.actions);
    };
    assert!(matches!(
        &request.action,
        crate::actions::Action::PaneClose(params) if params.pane_id == "pane_1"
    ));
}

#[test]
fn capability_refresh_does_not_replace_a_removed_highlight_or_forward_enter() {
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.rename".into(), "pane.close".into()]));
    state.open_pane_context_menu("pane_1".into(), 0, 0);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("pane context menu");
    };
    assert_eq!(
        menu.items()[menu.highlighted].action,
        ClientContextMenuAction::RenamePane
    );

    state.set_endpoint_methods(Some(vec!["pane.split".into()]));
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("menu must consume the pending Enter rather than forward it to the program");
    };
    assert!(
        menu.items().get(menu.highlighted).is_none(),
        "a removed action must not become a different selection"
    );
    let outcome = state.handle_input_bytes(b"\r");
    assert!(
        outcome.actions.is_empty(),
        "no replacement action: {:?}",
        outcome.actions
    );
    assert!(!outcome
        .requests
        .iter()
        .any(|request| matches!(request, ClientMessage::ClientShellPaneInput { .. })));
    assert!(state.overlay.is_none());
}

#[test]
fn capability_refresh_preserves_an_open_context_menu_for_an_equivalent_set() {
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.rename".into(), "pane.close".into()]));
    state.open_pane_context_menu("pane_1".into(), 0, 0);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!("pane context menu");
    };
    let close = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::ClosePane)
        .expect("close pane item");
    menu.highlighted = close;
    let before = menu
        .items()
        .iter()
        .map(|item| item.action)
        .collect::<Vec<_>>();

    // Same set in another order: nothing to re-gate, so nothing may reset.
    assert!(!state.set_endpoint_methods(Some(vec!["pane.close".into(), "pane.rename".into(),])));
    // A different set that still gates exactly the same items.
    assert!(state.set_endpoint_methods(Some(vec![
        "pane.rename".into(),
        "pane.close".into(),
        "workspace.list".into(),
    ])));

    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("the open context menu survives a capability refresh");
    };
    assert_eq!(
        menu.items()
            .iter()
            .map(|item| item.action)
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(menu.highlighted, close);
}

#[test]
fn local_selection_does_not_need_an_endpoint_activation_event() {
    let mut state = state_with_snapshot(snapshot());
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    tx.try_send(crate::client::ClientLoopEvent::Timer).unwrap();
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
            workspace_id: "ws_1".into(),
        }),
        &mut outcome,
    );
    assert!(
        matches!(&outcome.actions[..],[ClientShellAction::Endpoint {request,..}] if matches!(request.action,crate::actions::Action::WorkspaceFocus(_)))
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(crate::client::ClientLoopEvent::Timer)
    ));
}

#[test]
fn dispatcher_cancels_requests_when_the_endpoint_worker_fails() {
    use crate::client::connection::EndpointConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    {
        let mut state = state_with_snapshot(snapshot());
        state.set_pane_surface(surface());
        let actions = workspace_focus_actions(&mut state, "ws_1");
        let (handle, probe) = crate::client::core::test_support::endpoint();
        probe.fail(std::io::ErrorKind::BrokenPipe.into());
        let mut endpoints = EndpointConnection::new(handle);
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
        assert!(state
            .visible_endpoint_notice
            .as_ref()
            .is_some_and(|notice| { notice.title == "Action interrupted" }));
        assert_eq!(commands.diagnostic_snapshot()["queued"], 0);
    }
}

#[test]
fn old_boot_queued_request_is_cancelled_without_blocking_the_current_boot() {
    use crate::client::connection::EndpointConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    let actions = workspace_focus_actions(&mut state, "ws_stale");
    let stale_id = request_id(&actions).to_owned();
    let mut replacement = snapshot();
    replacement.boot_id = "replacement".into();
    state.set_snapshot(Box::new(replacement));
    let current = workspace_focus_actions(&mut state, "ws_1");
    let current_id = request_id(&current).to_owned();
    let mut commands = EndpointCommands::default();
    for action in actions.into_iter().chain(current) {
        let ClientShellAction::Endpoint { boot_id, request } = action else {
            panic!("expected endpoint request");
        };
        commands.enqueue(boot_id, request);
    }
    let (handle, probe) = crate::client::core::test_support::endpoint();
    let mut endpoints = EndpointConnection::new(handle);
    assert_eq!(commands.observe_snapshot("replacement"), [stale_id.clone()]);
    state.cancel_endpoint_request(&stale_id);
    assert!(commands.send_next(&mut endpoints).is_empty());
    assert!(!commands.accepts_response("boot-1", &stale_id));
    assert!(commands.accepts_response("replacement", &current_id));
    assert!(state.pending_requests.contains_key(&current_id));
    assert_eq!(probe.drain().len(), 1, "only the current request is queued");
    drop(probe);
}

#[test]
fn failed_selection_copy_does_not_send_terminal_input() {
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.selection = Some(crate::selection::Selection::absolute_range(
        "pane_1".into(),
        (0, 0),
        (0, 2),
    ));
    for result in [
        Ok(crate::actions::ActionResult::PaneSelection {
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
    let mut state = state_with_snapshot(snapshot());
    state.set_pane_surface(surface());
    state.compose(100, 28).unwrap();
    let pane_id = state.hits.panes[0].pane_id.clone();
    let inner_rect = state.hits.panes[0].inner_rect;
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method_with_kind(
        crate::actions::Action::PaneLinkActivate(crate::actions::PaneLinkActivateParams {
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

fn workspace_focus_actions(
    state: &mut ClientShellState,
    workspace_id: &str,
) -> Vec<ClientShellAction> {
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
            workspace_id: workspace_id.into(),
        }),
        &mut outcome,
    );
    outcome.actions
}
