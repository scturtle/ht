use super::*;

#[test]
fn retired_announcement_data_does_not_open_a_modal_or_disable_live_overlays() {
    let mut config = Config::default();
    config.keys.help = crate::config::BindingConfig::one("f2");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    assert!(state.overlay.is_none());
    let mut projection = snapshot();
    // The shared payload still has this field, but this Core-only UI no longer
    // implements its modal. Receiving it must not swallow ordinary pane input.
    projection.product_announcement = Some(crate::protocol::ClientShellProductAnnouncement {
        version: "legacy".into(),
        id: "unused".into(),
        title: "unused".into(),
        body: "unused".into(),
        preview: false,
    });
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(surface());
    assert!(state.overlay.is_none());
    let input = state.handle_input_bytes(b"x");
    assert!(matches!(
        input.requests.as_slice(),
        [ClientMessage::ClientShellPaneInput { pane_id, .. }] if pane_id == "pane_1"
    ));
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::F(2),
        KeyModifiers::NONE,
    ))]);
    assert_eq!(
        state.overlay.as_ref().map(ClientShellOverlay::kind),
        Some(ClientShellOverlayKind::Help)
    );
    assert!(state.compose(106, 30).is_some());
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    ))]);
    assert!(state.overlay.is_none());
}

#[test]
fn startup_config_diagnostics_are_client_rendered_and_persist_until_replaced() {
    let config = ClientShellConfig::from_config(&Config::default())
        .with_startup_config_diagnostic(Some("local config warning".into()));
    let mut state = ClientShellState::new(config);
    let mut shared_snapshot = snapshot();
    shared_snapshot.config_diagnostic = Some("local config warning".into());
    state.set_snapshot(Box::new(shared_snapshot));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("client + endpoint: local config warning")
    );

    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint config warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 20).expect("diagnostic frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("client: local config warning"));
    assert!(text.contains("endpoint: endpoint config warning"));

    state.handle_input_bytes(b"x");
    assert!(state.config_diagnostic.is_some());

    state.set_snapshot(Box::new(snapshot()));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("local config warning")
    );
}

#[test]
fn config_diagnostic_offsets_only_the_pane_rows_it_overlaps() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.toast_delay_seconds = 0;
    let mut state = ClientShellState::new(config);
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("one-line warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    state.visible_notification = Some(ClientVisibleNotification {
        event: SemanticNotification {
            kind: SemanticNotificationKind::Custom,
            title: "notification".into(),
            body: None,
            agent: None,
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            position: Some(crate::config::ToastHerdrPosition::TopRight),
        },
        deadline: std::time::Instant::now(),
    });

    state.compose(106, 20).expect("one-line frame");
    let pane_area = state.layout(106, 20).pane_surface;
    assert_eq!(state.hits.notification_toast.y, pane_area.y);
    let targetless_hit = state.hits.notification_toast;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: targetless_hit.x,
        row: targetless_hit.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.visible_notification.is_some());

    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("first warning\nsecond warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.compose(106, 20).expect("two-line frame");
    assert_eq!(state.hits.notification_toast.y, pane_area.y);

    state
        .visible_notification
        .as_mut()
        .expect("visible notification")
        .event
        .position = Some(crate::config::ToastHerdrPosition::BottomRight);
    state.compose(106, 20).expect("bottom notification frame");
    assert_eq!(state.hits.notification_toast.bottom(), 19);
}

#[test]
fn local_reload_keeps_key_diagnostics_and_ignores_retired_commands() {
    let initial: Config = toml::from_str(
        "[keys]\nprefix = 'ctrl+a'\n[[keys.command]]\nkey = 'f8'\ncommand = 'local-only'",
    )
    .unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&initial));
    let mut projection = snapshot();
    state.set_snapshot(Box::new(projection.clone()));
    assert_eq!(state.config.keybinds.prefix, initial.prefix_keys());
    assert!(state.endpoint_error.is_none());

    // Routine projection updates cannot replace the local keymap or reset an active prefix.
    state.mode = ClientShellMode::Prefix;
    projection.revision += 1;
    state.set_snapshot(Box::new(projection));
    assert_eq!(state.mode, ClientShellMode::Prefix);
    assert_eq!(state.config.keybinds.prefix, initial.prefix_keys());

    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let path = std::env::temp_dir().join(format!(
        "ht-local-keymap-reload-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let content = "[keys]\nprefix = 'ctrl+x'\nclose_pane = 'x'\n\
                   [[keys.command]]\nkey = 'f8'\ncommand = 'still-local-only'\n";
    std::fs::write(&path, content).unwrap();
    let previous = std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR);
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
    // Exercise the actual shell reload method, not just the config parser.
    state.reload_client_config();
    match previous {
        Some(value) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, value),
        None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
    }
    let unchanged = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();

    assert_eq!(unchanged, content);
    assert_eq!(
        state.config.keybinds.prefix,
        vec![(KeyCode::Char('x'), KeyModifiers::CONTROL)]
    );
    assert!(state
        .config
        .keybinds
        .keybinds
        .close_pane
        .bindings
        .is_empty());
    assert!(
        state.local_config_diagnostic.is_some(),
        "local key diagnostics must stay visible"
    );
    assert!(crate::input::resolve_direct_binding(
        &state.config.keybinds.keybinds,
        &crate::input::TerminalKey::new(KeyCode::F(8), KeyModifiers::empty()),
    )
    .is_none());
    assert!(state.endpoint_error.is_none());
}
