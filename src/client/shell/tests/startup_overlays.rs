use super::*;

#[test]
fn retired_announcement_data_does_not_open_a_modal_or_disable_live_overlays() {
    let mut config = Config::default();
    config.keys.help = crate::config::BindingConfig::one("f2");
    config.keys.settings = crate::config::BindingConfig::one("f3");
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
    for (code, kind) in [
        (KeyCode::F(2), ClientShellOverlayKind::Help),
        (KeyCode::F(3), ClientShellOverlayKind::Settings),
    ] {
        state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
            code,
            KeyModifiers::NONE,
        ))]);
        assert_eq!(
            state.overlay.as_ref().map(ClientShellOverlay::kind),
            Some(kind)
        );
        assert!(state.compose(106, 30).is_some());
        state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        ))]);
        assert!(state.overlay.is_none());
    }
}

#[test]
fn release_notes_reconcile_and_failed_dismiss_reopens_authoritative_snapshot() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.latest_release_notes_available = true;
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.3".into(),
        body: "first notes".into(),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot.clone()));
    state.open_release_notes();

    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.4".into(),
        body: "second notes".into(),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot.clone()));
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { ref version, .. }
        )) if version == "0.8.4"
    ));

    let dismissed = state.handle_input_bytes(b"\r");
    let request_id = match &dismissed.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        actions => panic!("unexpected actions: {actions:?}"),
    };
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.5".into(),
        body: "current notes".into(),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot));

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("stale_release_notes".into()),
            message: "dismiss failed".into(),
        }),
    );
    assert!(repaint);
    assert!(actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { ref version, .. }
        )) if version == "0.8.5"
    ));
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
            sound: None,
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
fn endpoint_reload_result_does_not_override_snapshot_diagnostic_authority() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.pending_requests.insert(
        "reload-1".into(),
        PendingEndpointRequest {
            boot_id: "boot-1".into(),
            method_name: "server.reload_config".into(),
            confirmation_workspace_id: None,
            kind: PendingEndpointKind::ReloadConfig,
        },
    );

    state.handle_endpoint_result(
        "boot-1",
        "reload-1",
        Ok(crate::api::schema::ResponseResult::ConfigReload {
            status: crate::config::ConfigReloadStatus::Partial,
            diagnostics: vec!["keybinding warning".into()],
        }),
    );
    assert_eq!(state.config_diagnostic.as_deref(), Some("endpoint warning"));

    state.set_snapshot(Box::new(snapshot()));
    assert!(state.config_diagnostic.is_none());
}

#[test]
fn local_reload_keeps_key_diagnostics_and_never_activates_unpublished_commands() {
    let initial: Config = toml::from_str(
        "[keys]\nprefix = 'ctrl+a'\n[[keys.command]]\nkey = 'f8'\ncommand = 'local-only'",
    )
    .unwrap();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&initial));
    assert!(state.config.keybinds.keybinds.custom_commands.is_empty());
    let mut projection = snapshot();
    projection.server_keybindings_toml = Some("not a keybinding profile".into());
    state.set_snapshot(Box::new(projection.clone()));
    assert_eq!(state.config.keybinds.prefix, initial.prefix_keys());
    assert!(state.config.keybinds.keybinds.custom_commands.is_empty());
    assert!(state.endpoint_error.is_none());

    // Legacy profile metadata has no authority over the local keymap or an
    // in-progress prefix. Manifest binding/identity changes are tested separately.
    state.mode = ClientShellMode::Prefix;
    projection.revision += 1;
    projection.server_keybindings_toml = Some("another ignored profile".into());
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
    assert!(state.config.keybinds.keybinds.custom_commands.is_empty());
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
    assert!(state.endpoint_error.is_none());
}

#[test]
fn live_client_config_keeps_sound_diagnostics() {
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    let mut config = Config::default();
    config.ui.sound.path = Some(std::path::PathBuf::from("invalid.wav"));

    let diagnostics = shell_config.apply_live_config(&config, &[], &[]);
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("expected an mp3 file")));
}

#[test]
fn update_ready_menu_opens_client_owned_release_notes_and_dismisses_by_version() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.update_available = Some("0.8.3".into());
    endpoint_snapshot.latest_release_notes_available = true;
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.3".into(),
        body: (0..40)
            .map(|index| format!("- release line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot.clone()));
    state.set_pane_surface(surface());
    let shell = state.compose(106, 30).expect("shell frame");
    assert_eq!(state.hits.global_launcher.width, 8);
    let launcher = state.hits.global_launcher;
    let shell_buffer = shell.to_ratatui_buffer().expect("shell buffer");
    let badge_x = launcher.right().saturating_sub(6);
    assert_eq!(
        shell_buffer[(badge_x, launcher.y)].fg,
        state.config.palette.accent
    );
    assert_eq!(
        shell_buffer[(badge_x + 2, launcher.y)].fg,
        state.config.palette.overlay0
    );

    state.sidebar_collapsed = true;
    let collapsed = state.compose(106, 30).expect("collapsed update shell");
    let collapsed_buffer = collapsed.to_ratatui_buffer().expect("collapsed buffer");
    assert_eq!(
        collapsed_buffer[(state.hits.sidebar_toggle.x, state.hits.sidebar_toggle.y)].fg,
        state.config.palette.accent
    );
    state.sidebar_collapsed = false;
    state.mode = ClientShellMode::Navigate;
    let navigate = state.compose(106, 30).expect("navigate update status");
    let navigate_text = navigate
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(navigate_text.contains("update ready"));
    state.mode = ClientShellMode::Prefix;
    let prefix = state
        .compose(106, 30)
        .expect("prefix without update status");
    let prefix_text = prefix
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(!prefix_text.contains("update ready"));
    state.mode = ClientShellMode::Navigate;

    state.toggle_global_menu();
    let menu = state.compose(106, 30).expect("update menu");
    let text = menu
        .cells
        .chunks(menu.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("● update ready"));
    let update_row = state.hits.global_menu_rows[3].0;
    // Other menu entries may be wider; the update badge must still fit.
    assert!(update_row.width >= 16);
    let menu_buffer = menu.to_ratatui_buffer().expect("menu buffer");
    assert_eq!(
        menu_buffer[(update_row.x + 1, update_row.y)].fg,
        state.config.palette.accent
    );
    assert_eq!(
        menu_buffer[(update_row.x + 3, update_row.y)].fg,
        state.config.palette.text
    );
    state.activate_global_menu_item(3, &mut ClientShellInput::default());
    let notes = state.compose(106, 30).expect("release notes");
    let bottom_row_start = usize::from(notes.width) * usize::from(notes.height - 1);
    let bottom_row = notes.cells[bottom_row_start..]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(!bottom_row.contains("NAVIGATE"));
    let text = notes
        .cells
        .chunks(notes.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("v0.8.3"));
    assert!(text.contains("update ready"));
    assert!(text.contains("upstream self-update"));
    assert!(text.contains("is disabled"));
    assert!(!text.contains("detach, run herdr update"));
    assert!(!state.hits.release_notes_scrollbar.is_empty());
    let outer = crate::ui::centered_popup_rect(
        Rect::new(0, 0, 106, 30),
        crate::ui::RELEASE_NOTES_MODAL_SIZE.0,
        crate::ui::RELEASE_NOTES_MODAL_SIZE.1,
    )
    .expect("release notes outer");
    let inner = Rect::new(outer.x + 1, outer.y + 1, outer.width - 2, outer.height - 2);
    let stack = crate::ui::modal_stack_areas(inner, 2, 1, 0, 1);
    assert_eq!(
        state.hits.overlay_primary,
        crate::ui::release_notes_close_button_rect(Rect::new(
            stack.header.x,
            stack.header.y,
            stack.header.width,
            1,
        ))
    );
    let notes_buffer = notes.to_ratatui_buffer().expect("release notes buffer");
    let title_cell = &notes_buffer[(stack.header.x + 1, stack.header.y)];
    assert_eq!(title_cell.fg, state.config.palette.text);
    assert!(title_cell.modifier.contains(Modifier::BOLD));
    assert_eq!(
        notes_buffer[(state.hits.overlay_primary.x, state.hits.overlay_primary.y)].bg,
        state.config.palette.accent
    );

    let outside =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })]);
    assert!(outside.requests.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(_))
    ));
    let pane_text = state.handle_raw_events(vec![
        RawInputEvent::Text(crate::input::TextCommit::new("ime")),
        RawInputEvent::Paste("secret".into()),
    ]);
    assert!(pane_text.requests.is_empty());

    let metrics = state
        .hits
        .release_notes_scroll_metrics
        .expect("release notes scroll metrics");
    let track = state.hits.release_notes_scrollbar;
    let thumb = crate::ui::scrollbar_thumb(metrics, track).expect("release notes thumb");
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: track.x,
        row: thumb.top,
        modifiers: KeyModifiers::NONE,
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: track.x,
        row: track.bottom().saturating_sub(1),
        modifiers: KeyModifiers::NONE,
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: track.x,
        row: track.bottom().saturating_sub(1),
        modifiers: KeyModifiers::NONE,
    })]);
    assert!(state.chrome_drag.is_none());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { scroll, .. }
        )) if usize::from(scroll) == state.hits.release_notes_max_scroll
    ));
    if let Some(ClientShellOverlay::ReleaseNotes(notes)) = state.overlay.as_mut() {
        notes.scroll = 0;
    }
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })]);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { scroll: 3, .. }
        ))
    ));
    let repeated = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::PageDown, KeyModifiers::empty())
            .with_kind(crossterm::event::KeyEventKind::Repeat),
    )]);
    assert!(repeated.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { scroll: 3, .. }
        ))
    ));
    let dismissed = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none());
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(matches!(
        &dismissed.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::ReleaseNotesDismiss(params)
                    if params.version == "0.8.3"
            )
    ));

    endpoint_snapshot.boot_id = "boot-2".into();
    endpoint_snapshot.revision = 2;
    endpoint_snapshot.update_available = None;
    endpoint_snapshot
        .release_notes
        .as_mut()
        .expect("release notes")
        .preview = false;
    state.set_snapshot(Box::new(endpoint_snapshot));
    let mut installed_surface = surface();
    installed_surface.boot_id = "boot-2".into();
    installed_surface.projection_revision = 2;
    state.set_pane_surface(installed_surface);
    state.compose(106, 30).expect("installed shell");
    assert_eq!(state.hits.global_launcher.width, 6);
    state.toggle_global_menu();
    let installed = state.compose(106, 30).expect("installed menu");
    let installed_text = installed
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(installed_text.contains("what's new"));
    assert!(!installed_text.contains("● what's new"));
}

#[test]
fn coalesced_release_notes_open_and_scroll_uses_current_geometry() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.update_available = Some("0.8.3".into());
    endpoint_snapshot.latest_release_notes_available = true;
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.3".into(),
        body: (0..40)
            .map(|index| format!("- release line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("initial shell");
    state.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: 3,
    }));

    state.handle_raw_events(vec![
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Enter,
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::PageDown,
            KeyModifiers::empty(),
        )),
    ]);

    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { scroll: 8, .. }
        ))
    ));
}

#[test]
fn coalesced_release_notes_open_and_mouse_uses_current_geometry() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let body = (0..40)
        .map(|index| format!("- release line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.update_available = Some("0.8.3".into());
    endpoint_snapshot.latest_release_notes_available = true;
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.3".into(),
        body: body.clone(),
        preview: true,
    });
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("initial shell");

    let outer = crate::ui::centered_popup_rect(
        Rect::new(0, 0, 106, 30),
        crate::ui::RELEASE_NOTES_MODAL_SIZE.0,
        crate::ui::RELEASE_NOTES_MODAL_SIZE.1,
    )
    .expect("release notes outer");
    let inner = Rect::new(outer.x + 1, outer.y + 1, outer.width - 2, outer.height - 2);
    let stack = crate::ui::modal_stack_areas(inner, 2, 1, 0, 1);
    let notes = crate::app::state::ReleaseNotesState {
        version: "0.8.3".into(),
        body,
        scroll: 0,
        preview: true,
    };
    let metrics = crate::ui::release_notes_scroll_metrics(
        &notes,
        "herdr update",
        stack.content,
        &state.config.palette,
    );
    let track = crate::ui::release_notes_scrollbar_rect(stack.content, metrics)
        .expect("release notes track");
    let close = crate::ui::release_notes_close_button_rect(Rect::new(
        stack.header.x,
        stack.header.y,
        stack.header.width,
        1,
    ));

    state.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: 3,
    }));
    state.handle_raw_events(vec![
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Enter,
            KeyModifiers::empty(),
        )),
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: track.x,
            row: track.bottom().saturating_sub(1),
            modifiers: KeyModifiers::NONE,
        }),
    ]);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ReleaseNotes(
            crate::app::state::ReleaseNotesState { scroll, .. }
        )) if scroll > 0
    ));

    state.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: 3,
    }));
    let closed = state.handle_raw_events(vec![
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Enter,
            KeyModifiers::empty(),
        )),
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: close.x,
            row: close.y,
            modifiers: KeyModifiers::NONE,
        }),
    ]);
    assert!(state.overlay.is_none());
    assert!(matches!(
        &closed.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(request.method, crate::api::schema::Method::ReleaseNotesDismiss(_))
    ));
}

#[test]
fn current_release_notes_use_whats_new_without_attention_badge() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.latest_release_notes_available = true;
    endpoint_snapshot.release_notes = Some(crate::protocol::ClientShellReleaseNotes {
        version: "0.8.2".into(),
        body: "### Changed\n- Client shell".into(),
        preview: false,
    });
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("shell frame");
    assert_eq!(state.hits.global_launcher.width, 6);
    state.toggle_global_menu();
    let menu = state.compose(106, 30).expect("what's new menu");
    let text = menu
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("what's new"));
}
