use super::*;

fn agents() -> ClientShellSnapshot {
    let mut projected = snapshot();
    for (id, status, sequence) in [
        ("pane_1", AgentStatus::Working, 30),
        ("pane_2", AgentStatus::Working, 20),
        ("pane_3", AgentStatus::Blocked, 10),
    ] {
        if id != "pane_1" {
            let mut pane = projected.panes[0].clone();
            pane.pane_id = id.into();
            projected.panes.push(pane);
        }
        projected.agents.push(ClientShellAgent {
            pane_id: id.into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: None,
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: sequence,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        });
    }
    projected
}

#[test]
fn workspace_chooser_omits_the_menu_on_desktop_and_mobile() {
    for width in [106, 44] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface());
        let open = state.handle_input_bytes(b"\x02w");
        assert!(open.actions.is_empty() && open.requests.is_empty());
        let frame = state.compose(width, 40).expect("workspace chooser");
        let rows = frame_rows(&frame);
        assert!(rows.iter().any(|row| row.trim() == "spaces"));
        for retired in [
            "menu",
            "settings",
            "keybinds",
            "reload config",
            "dump pane diagnostics",
            "detach",
        ] {
            assert!(
                !rows.iter().any(|row| row.trim() == retired),
                "retired {retired}"
            );
        }
        assert!(state.hits.mobile_targets.iter().all(|(_, target)| matches!(
            target,
            ClientMobileTarget::Workspace { .. }
                | ClientMobileTarget::Tab { .. }
                | ClientMobileTarget::Agent { .. }
                | ClientMobileTarget::NewWorkspace
                | ClientMobileTarget::NewTab
        )));
    }
}

#[test]
fn navigator_ignores_retired_search_filter_and_text_input_without_pty_delivery() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(agents()));
    state.set_pane_surface(surface());
    state.open_navigator_overlay();
    state.compose(106, 30).expect("navigator");
    let targets = state.hits.navigator_rows.clone();
    assert_eq!(targets.len(), 4);
    for event in [
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('/'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('b'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('w'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('i'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('d'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Char('a'),
            KeyModifiers::empty(),
        )),
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Backspace,
            KeyModifiers::empty(),
        )),
        RawInputEvent::Text(crate::input::TextCommit::new("needle界")),
        RawInputEvent::Paste("needle界".into()),
    ] {
        let result = state.handle_raw_events(vec![event]);
        assert!(result.actions.is_empty() && result.requests.is_empty());
        let frame = state.compose(106, 30).expect("navigator remains open");
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::Navigator(_))
        ));
        assert!(
            frame.cursor.is_none(),
            "retired search must not acquire a cursor"
        );
        assert_eq!(state.hits.navigator_rows, targets);
        let text = frame_rows(&frame).join("\n");
        assert!(!text.contains("search") && !text.contains("filter"));
    }
    let close = state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
    assert!(close.actions.is_empty() && close.requests.is_empty());
}

#[test]
fn disconnected_navigator_keeps_cached_rows_without_focus_requests() {
    for workspace in [false, true] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(agents()));
        state.mark_endpoint_disconnected();
        state.open_navigator_overlay();
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("navigator");
        };
        let rows = super::super::navigator::navigator_rows(&state.endpoint);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.stale));
        navigator.selected = Some(if workspace {
            ClientNavigatorTarget::Workspace {
                workspace_id: "ws_1".into(),
            }
        } else {
            ClientNavigatorTarget::Pane {
                pane_id: "pane_2".into(),
            }
        });
        let mut outcome = ClientShellInput::default();
        state.accept_navigator_selection(&mut outcome);
        assert!(outcome.actions.is_empty() && outcome.requests.is_empty());
        assert!(outcome.repaint);
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::Navigator(_))
        ));
        assert!(state.visible_endpoint_notice.is_some());
        assert_eq!(state.focused_pane_id().as_deref(), Some("pane_1"));
    }
}

#[test]
fn unsupported_navigator_focus_preserves_its_existing_dismissal_policy() {
    for workspace in [false, true] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot()));
        state.set_endpoint_methods(Some(Vec::new()));
        state.open_navigator_overlay();
        let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
            panic!("navigator");
        };
        navigator.selected = Some(if workspace {
            ClientNavigatorTarget::Workspace {
                workspace_id: "ws_1".into(),
            }
        } else {
            ClientNavigatorTarget::Pane {
                pane_id: "pane_1".into(),
            }
        });
        let mut outcome = ClientShellInput::default();
        state.accept_navigator_selection(&mut outcome);
        assert!(outcome.actions.is_empty());
        assert!(state.pending_requests.is_empty());
        assert!(state.overlay.is_none());
        assert!(state.visible_endpoint_notice.is_some());
    }
}

#[test]
fn mobile_focus_uses_common_admission_for_workspace_agent_and_tab() {
    for disconnected in [false, true] {
        for kind in 0..3 {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.set_snapshot(Box::new(agents()));
            state.set_pane_surface(surface());
            state.set_endpoint_methods(Some(Vec::new()));
            if disconnected {
                state.mark_endpoint_disconnected();
            }
            state.mode = ClientShellMode::Navigate;
            state.navigate_workspace_id = state.focused_navigation_target();
            state.compose(44, 90).unwrap();
            let rect = state
                .hits
                .mobile_targets
                .iter()
                .find_map(|(rect, target)| {
                    matches!(
                        (kind, target),
                        (0, ClientMobileTarget::Workspace { .. })
                            | (1, ClientMobileTarget::Agent { .. })
                            | (2, ClientMobileTarget::Tab { .. })
                    )
                    .then_some(*rect)
                })
                .unwrap();
            let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(outcome.actions.is_empty() && outcome.requests.is_empty());
            assert!(state.pending_requests.is_empty());
            assert_eq!(
                state.mode,
                if disconnected {
                    ClientShellMode::Navigate
                } else {
                    ClientShellMode::Terminal
                }
            );
            assert_eq!(state.navigate_workspace_id.is_some(), disconnected);
            assert!(state.visible_endpoint_notice.is_some());
        }
    }
}

#[test]
fn mobile_cached_agent_order_preserves_priority_recency_and_filtered_views() {
    use crate::config::AgentPanelSortConfig::{Priority, Spaces};
    for (sort, filtered, tied_recency, expected) in [
        (Spaces, false, false, vec!["pane_1", "pane_2", "pane_3"]),
        (Priority, false, false, vec!["pane_3", "pane_2", "pane_1"]),
        (Spaces, true, false, vec!["pane_2", "pane_1"]),
        (Priority, true, false, vec!["pane_2", "pane_1"]),
        (Priority, true, true, vec!["pane_2", "pane_1"]),
    ] {
        for disconnected in [false, true] {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.config.agent_panel_sort = sort;
            let mut projected = agents();
            if filtered {
                projected.agent_view_label = Some("selected".into());
                projected.agent_order = vec!["pane_2".into(), "pane_1".into()];
            }
            state.set_snapshot(Box::new(projected));
            state.set_pane_surface(surface());
            // Arrival recency is not necessarily the raw state-change sequence.
            state.endpoint.agent_recency = [
                ("pane_1".into(), 1),
                ("pane_2".into(), if tied_recency { 1 } else { 2 }),
            ]
            .into_iter()
            .collect();
            if disconnected {
                state.mark_endpoint_disconnected();
            }
            state.mode = ClientShellMode::Navigate;
            let frame = state.compose(44, 90).expect("mobile switcher");
            let actual = state
                .hits
                .mobile_targets
                .iter()
                .filter_map(|(_, target)| match target {
                    ClientMobileTarget::Agent { pane_id } => Some(pane_id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                actual, expected,
                "{sort:?}, filtered={filtered}, disconnected={disconnected}"
            );
            assert!(state.hits.mobile_targets.iter().any(|(_, target)| {
                matches!(target, ClientMobileTarget::Workspace { workspace_id } if workspace_id == "ws_1")
            }));
            if disconnected {
                assert!(frame
                    .cells
                    .iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
                    .contains("reconnecting"));
                let rect = state
                    .hits
                    .mobile_targets
                    .iter()
                    .find_map(|(rect, target)| {
                        matches!(target, ClientMobileTarget::Agent { .. }).then_some(*rect)
                    })
                    .unwrap();
                let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: rect.x,
                    row: rect.y,
                    modifiers: KeyModifiers::empty(),
                })]);
                assert!(outcome.actions.is_empty());
                assert_eq!(state.mode, ClientShellMode::Navigate);
                assert!(state.visible_endpoint_notice.is_some());
            }
        }
    }
}
