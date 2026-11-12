use super::*;

#[test]
fn a_new_projection_epoch_accepts_restarted_revisions_but_its_stale_updates_do_not() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut old = snapshot();
    old.revision = 80;
    assert!(state.set_endpoint_snapshot_for_generation(1, Box::new(old)));
    let mut paint = surface();
    paint.projection_revision = 80;
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    let mut fresh = snapshot();
    fresh.boot_id = "replacement-projection".into();
    assert!(state.set_endpoint_snapshot_for_generation(1, Box::new(fresh.clone())));
    assert_eq!(state.endpoint.snapshot.as_ref().unwrap().revision, 1);
    let mut paint = surface();
    paint.boot_id = fresh.boot_id.clone();
    state.set_pane_surface(paint);
    assert!(state.compose(100, 28).is_some());
    fresh.revision = 0;
    assert!(!state.set_endpoint_snapshot_for_generation(1, Box::new(fresh)));
    assert_eq!(state.endpoint.snapshot.as_ref().unwrap().revision, 1);
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

fn request_id(actions: &[ClientShellAction]) -> &str {
    let [ClientShellAction::Endpoint { request, .. }] = actions else {
        panic!("expected one endpoint request");
    };
    &request.id
}

struct TestTransport {
    fail: bool,
}

impl crate::client::connection::CoreTransport for TestTransport {
    fn send(&mut self, _: &ClientMessage) -> std::io::Result<()> {
        if self.fail {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn capability_shrink_after_recovery_blocks_a_stale_close_confirmation() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.confirm_close = true;
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["tab.close".into()]));
    let boot_id = state.endpoint.snapshot.as_ref().unwrap().boot_id.clone();
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::actions::Action::TabClose(crate::actions::TabTarget {
            tab_id: "tab_1".into(),
        }),
        &mut outcome,
    );
    let request_id = request_id(&outcome.actions).to_owned();
    state.handle_endpoint_result(
        &boot_id,
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("confirmation_required".into()),
            message: "confirm".into(),
        }),
    );
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ConfirmClose(_))
    ));

    // The relay recovered onto a Core that implements fewer methods.
    state.set_endpoint_methods(Some(vec![]));
    let mut confirm = ClientShellInput::default();
    state.accept_close_confirmation(&mut confirm);

    assert!(confirm.actions.is_empty());
    assert!(state.pending_requests.is_empty());
    assert!(state
        .visible_endpoint_notice
        .as_ref()
        .is_some_and(|notice| notice.title == "Action unavailable"));
}

#[test]
fn capability_refresh_preserves_pending_request_boot_fences() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
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
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "pane.rename".into(),
        "pane.split".into(),
        "pane.zoom".into(),
        "pane.close".into(),
    ]));
    state.open_pane_context_menu("pane_1".into(), 0, 0);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!("pane context menu");
    };
    let split_right = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::SplitRight)
        .expect("split right item");
    menu.highlighted = split_right;
    assert!(menu
        .items()
        .iter()
        .any(|item| item.action == ClientContextMenuAction::RenamePane));
    state.compose(106, 20).expect("pane context menu rows");
    assert!(!state.hits.context_menu_rows.is_empty());

    // The replacement Core lost the optional pane.rename ability that Core only
    // advertises while it reports terminal.label support.
    assert!(state.set_endpoint_methods(Some(vec![
        "pane.split".into(),
        "pane.zoom".into(),
        "pane.close".into(),
    ])));

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
        ClientContextMenuAction::SplitRight
    );
    assert!(
        state.hits.context_menu_rows.is_empty(),
        "rows laid out for the old item list must not stay clickable"
    );

    let highlighted = menu.highlighted;
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(highlighted, &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("split request: {:?}", outcome.actions);
    };
    assert!(matches!(
        &request.action,
        crate::actions::Action::PaneSplit(params)
            if params.direction == crate::actions::SplitDirection::Right
    ));
}

#[test]
fn capability_refresh_does_not_replace_a_removed_highlight_or_forward_enter() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.rename".into(), "pane.split".into()]));
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
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec![
        "pane.rename".into(),
        "pane.split".into(),
        "pane.zoom".into(),
        "pane.close".into(),
    ]));
    state.open_pane_context_menu("pane_1".into(), 0, 0);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_mut() else {
        panic!("pane context menu");
    };
    let split_down = menu
        .items()
        .iter()
        .position(|item| item.action == ClientContextMenuAction::SplitDown)
        .expect("split down item");
    menu.highlighted = split_down;
    let before = menu
        .items()
        .iter()
        .map(|item| item.action)
        .collect::<Vec<_>>();

    // Same set in another order: nothing to re-gate, so nothing may reset.
    assert!(!state.set_endpoint_methods(Some(vec![
        "pane.close".into(),
        "pane.zoom".into(),
        "pane.rename".into(),
        "pane.split".into(),
    ])));
    // A different set that still gates exactly the same items.
    assert!(state.set_endpoint_methods(Some(vec![
        "pane.rename".into(),
        "pane.split".into(),
        "pane.zoom".into(),
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
    assert_eq!(menu.highlighted, split_down);
}

#[test]
fn capability_refresh_regates_an_open_settings_overlay() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    // Capabilities are still unknown, so the client-local toast section is offered.
    state.open_settings_overlay();
    state.select_settings_section(ClientSettingsSection::Theme, &mut outcome);
    state.move_settings_selection(1);
    state.select_settings_section(ClientSettingsSection::Toast, &mut outcome);
    state.move_settings_selection(2);
    let previewed_theme = state.config.theme_name.clone();
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("settings overlay");
    };
    assert_eq!(settings.sections, ClientSettingsSection::ALL.to_vec());
    assert_eq!(settings.section, ClientSettingsSection::Toast);
    assert_eq!(settings.selected, 2);
    let original_theme = settings.original_theme_name.clone();
    let original_palette = settings.original_palette.clone();

    // The recovered Core advertises methods, so semantic toast options are hidden.
    assert!(state.set_endpoint_methods(Some(vec!["pane.focus".into()])));

    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("the open settings overlay survives a capability refresh");
    };
    assert_eq!(
        settings.sections,
        vec![
            ClientSettingsSection::Theme,
            ClientSettingsSection::Indicators,
            ClientSettingsSection::Sound
        ]
    );
    assert_eq!(settings.section, ClientSettingsSection::Theme);
    let previewed_index = crate::config::THEME_NAMES
        .iter()
        .position(|name| {
            super::settings::normalized_theme_name(name)
                == super::settings::normalized_theme_name(&previewed_theme)
        })
        .expect("previewed theme is a known theme");
    assert_eq!(settings.selected, previewed_index);
    // The preview and the cancel baseline still belong to the user's edits.
    assert_eq!(state.config.theme_name, previewed_theme);
    assert_eq!(settings.original_theme_name, original_theme);
    assert_eq!(settings.original_palette, original_palette);
}

#[test]
fn capability_refresh_preserves_open_settings_choice_when_sections_stay_equal() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    let mut outcome = ClientShellInput::default();
    state.open_settings_overlay();
    state.select_settings_section(ClientSettingsSection::Indicators, &mut outcome);
    state.move_settings_selection(1);
    let sections = ClientSettingsSection::ALL[..3].to_vec();
    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("settings overlay");
    };
    assert_eq!(settings.sections, sections);
    assert_eq!(settings.section, ClientSettingsSection::Indicators);
    assert_eq!(settings.selected, 1);

    // A different method set that gates the same settings sections.
    assert!(state.set_endpoint_methods(Some(vec!["workspace.list".into(), "pane.zoom".into(),])));

    let Some(ClientShellOverlay::Settings(settings)) = state.overlay.as_ref() else {
        panic!("the open settings overlay survives a capability refresh");
    };
    assert_eq!(settings.sections, sections);
    assert_eq!(settings.section, ClientSettingsSection::Indicators);
    assert_eq!(settings.selected, 1);
}

#[test]
fn local_selection_does_not_need_an_endpoint_activation_event() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
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
fn dispatcher_cancels_requests_when_the_core_transport_fails() {
    use crate::client::connection::CoreConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    {
        let fail_send = true;
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface());
        let actions = workspace_focus_actions(&mut state, "ws_1");
        let mut endpoints = CoreConnection::new(TestTransport { fail: fail_send }, 1);
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
fn stale_queued_request_is_cancelled_without_blocking_the_current_generation() {
    use crate::client::connection::CoreConnection;
    use crate::client::endpoint_commands::EndpointCommands;

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let actions = workspace_focus_actions(&mut state, "ws_stale");
    let stale_id = request_id(&actions).to_owned();
    let current = workspace_focus_actions(&mut state, "ws_1");
    let current_id = request_id(&current).to_owned();
    let mut commands = EndpointCommands::default();
    for (generation, actions) in [(1, actions), (2, current)] {
        for action in actions {
            let ClientShellAction::Endpoint { boot_id, request } = action else {
                panic!("expected endpoint request");
            };
            commands.enqueue(generation, boot_id, request);
        }
    }
    let mut endpoints = CoreConnection::new(TestTransport { fail: false }, 2);
    let cancelled = commands.send_next(&mut endpoints);
    assert_eq!(cancelled, vec![stale_id.clone()]);
    state.cancel_endpoint_request(&stale_id);
    assert!(!commands.accepts_response(1, "boot-1", &stale_id));
    assert!(commands.accepts_response(2, "boot-1", &current_id));
    assert!(state.pending_requests.contains_key(&current_id));
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
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
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
