use super::*;
use ratatui::layout::Position;

#[test]
fn tab_controls_and_hits_stay_inside_narrow_nonzero_origin_rectangles() {
    let projected = snapshot();
    let config = ClientShellConfig::from_config(&Config::default());
    for width in 0..=65 {
        for height in [0, 1] {
            let area = Rect::new(7, 3, width, height);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 84, 8));
            let before = buffer.clone();
            let mut hits = ShellHitMap::default();
            render::render_tab_bar(
                &mut buffer,
                area,
                &projected,
                &config,
                &mut 0,
                &mut true,
                None,
                &mut hits,
            );
            assert!(hits.mobile_switch.is_empty());
            assert!(hits.global_launcher.is_empty());
            for rect in std::iter::once(hits.mobile_switch)
                .chain(std::iter::once(hits.global_launcher))
                .chain(std::iter::once(hits.new_tab))
                .chain(hits.tabs.iter().map(|(rect, _)| *rect))
            {
                if !rect.is_empty() {
                    assert_eq!(rect.intersection(area), rect, "{area:?}: {rect:?}");
                }
            }
            for y in 0..8 {
                for x in 0..84 {
                    if !area.contains(Position::new(x, y)) {
                        assert_eq!(
                            buffer[(x, y)],
                            before[(x, y)],
                            "paint escaped {area:?} at {x},{y}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn desktop_workspace_selector_does_not_depend_on_a_pane_surface() {
    for connected in [false, true] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        if connected {
            state.set_snapshot(Box::new(snapshot()));
        }
        assert!(state.pane_surface.is_none());
        let opened = state.handle_input_bytes(b"\x02w");
        assert!(opened.actions.is_empty() && opened.requests.is_empty());
        assert_eq!(state.mode, ClientShellMode::Navigate);
        state
            .compose(106, 30)
            .expect("selector without terminal paint");
        assert!(!state.hits.mobile_close.is_empty());
        assert_eq!(
            state
                .hits
                .mobile_targets
                .iter()
                .any(|(_, t)| matches!(t, ClientMobileTarget::Workspace { .. })),
            connected
        );
        let closed = state.handle_input_bytes(b"\x1b");
        assert!(closed.actions.is_empty() && closed.requests.is_empty());
        assert_eq!(state.mode, ClientShellMode::Terminal);
    }
}

fn click(state: &mut ClientShellState, rect: Rect, button: MouseButton) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(button),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn desktop_surface_uses_all_columns_with_top_bottom_or_hidden_tab_bar() {
    for position in [TabBarPositionConfig::Top, TabBarPositionConfig::Bottom] {
        for hidden in [false, true] {
            for mouse in [false, true] {
                let mut config = Config::default();
                config.ui.tab_bar_position = position;
                config.ui.hide_tab_bar_when_single_tab = hidden;
                config.ui.mouse_capture = mouse;
                let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
                state.set_snapshot(Box::new(snapshot()));
                state.set_pane_surface(surface());
                let layout = state.layout(106, 30);
                assert_eq!(layout.pane_surface.x, 0);
                assert_eq!(layout.pane_surface.width, 106);
                assert_eq!(layout.pane_surface.height, 30 - u16::from(!hidden));
                let frame = state.compose(106, 30).expect("desktop frame");
                assert_eq!(state.hits.panes[0].rect.x, 0);
                assert_eq!(frame.width, 106);
                assert!(state.hits.global_launcher.is_empty());
                assert!(state.hits.mobile_switch.is_empty());
                if !hidden && mouse {
                    assert_eq!(state.hits.tabs.first().expect("first tab").0.x, 0);
                    for (rect, _) in &state.hits.tabs {
                        assert!(rect.right() <= 106);
                    }
                }
                // The pane's original first cell is now at the left edge, not x=26.
                let index = usize::from(layout.pane_surface.y) * usize::from(frame.width);
                assert_eq!(frame.cells[index].symbol, "L");
            }
        }
    }
}

#[test]
fn desktop_keyboard_selector_keeps_context_targets_stable() {
    let mut projected = snapshot();
    let mut second = projected.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.label = "SECOND SPACE".into();
    second.focused = false;
    projected.workspaces.push(second);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("desktop");
    assert!(state.hits.mobile_switch.is_empty());
    assert!(state.hits.global_launcher.is_empty());
    let opened = state.handle_input_bytes(b"\x02w");
    assert!(opened.repaint && opened.actions.is_empty() && opened.requests.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.compose(106, 30).expect("selector");
    let second = state.hits.mobile_targets.iter().find_map(|(rect, target)| {
        matches!(target, ClientMobileTarget::Workspace { workspace_id } if workspace_id == "ws_2").then_some(*rect)
    }).expect("second workspace is visible");
    let context = click(&mut state, second, MouseButton::Right);
    assert!(context.repaint && context.actions.is_empty() && context.requests.is_empty());
    assert!(
        matches!(state.overlay.as_ref(), Some(ClientShellOverlay::ContextMenu(menu))
        if matches!(&menu.target, ClientContextMenuTarget::Workspace { workspace_id, .. } if workspace_id == "ws_2"))
    );
    state.handle_input_bytes(b"\x1b");
    state
        .compose(106, 30)
        .expect("selector after context close");
    let focused = click(&mut state, second, MouseButton::Left);
    assert!(focused.requests.is_empty());
    assert!(
        matches!(focused.actions.as_slice(), [ClientShellAction::Endpoint { request, .. }]
        if matches!(&request.action, crate::actions::Action::WorkspaceFocus(target) if target.workspace_id == "ws_2"))
    );
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn hidden_tab_bar_still_has_the_keyboard_workspace_and_menu_entries() {
    let mut config = Config::default();
    config.ui.hide_tab_bar_when_single_tab = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("no tab bar");
    let open = state.handle_input_bytes(b"\x02w");
    assert!(open.actions.is_empty() && open.requests.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.compose(106, 30).expect("desktop keyboard chooser");
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, t)| matches!(t, ClientMobileTarget::NewWorkspace)));
    assert!(state
        .hits
        .mobile_targets
        .iter()
        .any(|(_, t)| matches!(t, ClientMobileTarget::Menu(_))));
    let cancel = state.handle_input_bytes(b"\x1b");
    assert!(cancel.actions.is_empty() && cancel.requests.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn workspace_rows_keep_duplicate_parents_and_linked_children_without_collapsing() {
    let mut projected = snapshot();
    let template = projected.workspaces[0].clone();
    projected.workspaces = [false, true, false]
        .into_iter()
        .enumerate()
        .map(|(index, linked)| {
            let mut workspace = template.clone();
            workspace.workspace_id = format!("ws_{}", index + 1);
            workspace.worktree = Some(ClientShellWorktree {
                key: "repo".into(),
                label: "repo".into(),
                is_linked_worktree: linked,
            });
            workspace
        })
        .collect();
    let entries = navigation_rows::workspace_entries(&projected);
    assert_eq!(
        entries.iter().map(|e| e.index).collect::<Vec<_>>(),
        [0, 2, 1]
    );
    assert!(!entries[0].indented && !entries[1].indented);
    assert!(entries[2].indented && entries[2].last_child);
    assert!(!navigation_rows::workspace_close_is_group(
        &projected,
        &projected.workspaces[0]
    ));
    projected.workspaces.remove(2);
    assert!(navigation_rows::workspace_close_is_group(
        &projected,
        &projected.workspaces[0]
    ));
    assert!(!navigation_rows::workspace_close_is_group(
        &projected,
        &projected.workspaces[1]
    ));
}
