use super::*;
use crate::actions::Action;
use crossterm::event::{MouseButton, MouseEventKind};

fn grouped_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut next_tab = projected.tabs[0].clone();
    next_tab.tab_id = "tab_2".into();
    next_tab.number = 8; // Stable identity, not display order.
    next_tab.label = "2".into();
    next_tab.focused = false;
    projected.tabs.push(next_tab);
    let mut second = projected.panes[0].clone();
    second.pane_id = "pane_2".into();
    second.label = Some("test".into());
    second.focused = false;
    projected.panes.push(second.clone());
    second.pane_id = "pane_3".into();
    second.tab_id = "tab_2".into();
    projected.panes.push(second);
    projected
}

fn mouse(rect: Rect, kind: MouseEventKind) -> RawInputEvent {
    RawInputEvent::Mouse(MouseEvent {
        kind,
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })
}

fn methods(outcome: &ClientShellInput) -> Vec<&Action> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.action),
            _ => None,
        })
        .collect()
}

#[test]
fn pane_entries_share_a_tab_group_but_clicks_target_the_exact_pane() {
    for (cols, bar_row) in [(106, 23), (56, 1)] {
        let mut state = state_with_snapshot(grouped_snapshot());
        state.set_pane_surface(surface());
        let frame = state.compose(cols, 24).unwrap();
        let rows = frame_rows(&frame);
        let bar = &rows[bar_row];
        assert!(
            bar.contains("1.1") && bar.contains("1.2 test") && bar.contains('│'),
            "{bar}"
        );
        assert!(bar.split_whitespace().any(|s| s == "2"), "{bar}");
        assert_eq!(state.hits.tabs.len(), 2);
        let [(first, first_id), (second, second_id)] = state.hits.tab_panes.as_slice() else {
            panic!("missing pane entries");
        };
        assert_eq!(
            (first_id.as_str(), second_id.as_str()),
            ("pane_1", "pane_2")
        );
        assert_eq!(state.hits.tabs[0].0.right(), second.right());
        let style = |rect: Rect| {
            frame.cells[usize::from(rect.y) * usize::from(cols) + usize::from(rect.x)].bg
        };
        assert_ne!(style(*first), style(*second));
        let outcome = state.handle_raw_events(vec![
            mouse(*second, MouseEventKind::Down(MouseButton::Left)),
            mouse(*second, MouseEventKind::Up(MouseButton::Left)),
        ]);
        assert!(
            matches!(methods(&outcome).as_slice(), [Action::PaneFocus(p)] if p.pane_id == "pane_2")
        );
    }
}

#[test]
fn mobile_second_row_lists_and_focuses_single_pane_tabs() {
    let mut projected = grouped_snapshot();
    projected.panes.retain(|pane| pane.pane_id != "pane_2");
    let mut state = state_with_snapshot(projected);
    state.set_pane_surface(surface());

    let frame = state.compose(44, 24).unwrap();
    let rows = frame_rows(&frame);
    let bar = &rows[1];
    assert!(bar.split_whitespace().any(|label| label == "1"), "{bar}");
    assert!(bar.split_whitespace().any(|label| label == "2"), "{bar}");
    assert!(bar.contains("switch"), "{bar}");
    let [(first, _), (second, _)] = state.hits.tabs.as_slice() else {
        panic!("missing single-pane tabs");
    };
    assert_eq!((first.y, second.y), (1, 1));
    let outcome = state.handle_raw_events(vec![
        mouse(*second, MouseEventKind::Down(MouseButton::Left)),
        mouse(*second, MouseEventKind::Up(MouseButton::Left)),
    ]);
    assert!(matches!(methods(&outcome).as_slice(), [Action::TabFocus(p)] if p.tab_id == "tab_2"));
}

#[test]
fn mobile_compacts_and_scrolls_even_when_all_tabs_have_one_pane() {
    let mut projected = snapshot();
    for index in 2..=12 {
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = format!("tab_{index}");
        tab.label = format!("Long name {index}");
        tab.custom_label = true;
        tab.number = index;
        tab.focused = index == 12;

        let mut pane = projected.panes[0].clone();
        pane.pane_id = format!("pane_{index}");
        pane.tab_id = tab.tab_id.clone();
        pane.focused = tab.focused;
        projected.panes.push(pane);
        projected.tabs.push(tab);
    }
    projected.tabs[0].focused = false;
    projected.panes[0].focused = false;
    projected.focused_tab_id = Some("tab_12".into());
    projected.focused_pane_id = Some("pane_12".into());
    projected.workspaces[0].active_tab_id = "tab_12".into();
    let mut state = state_with_snapshot(projected);
    state.set_pane_surface(surface());
    let frame = state.compose(29, 24).unwrap();
    let rows = frame_rows(&frame);
    assert!(rows[1].contains("12"), "{}", rows[1]);
    assert!(!rows[1].contains("Long name"));
    assert!(!state.hits.tab_scroll_left.is_empty());
    assert!(!state.hits.tab_scroll_right.is_empty());
    assert!(state.hits.tabs.iter().any(|(_, id)| id == "tab_12"));
    assert!(!state.hits.tabs.iter().any(|(_, id)| id == "tab_1"));
}

#[test]
fn grouped_menu_closes_only_the_clicked_pane() {
    let mut state = state_with_snapshot(grouped_snapshot());
    state.set_pane_surface(surface());
    state.compose(106, 24).unwrap();
    let rect = state.hits.tab_panes[1].0;
    state.handle_raw_events(vec![mouse(rect, MouseEventKind::Down(MouseButton::Right))]);
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("missing context menu");
    };
    let items = menu.items();
    let index = items
        .iter()
        .position(|item| item.action == ClientContextMenuAction::ClosePane)
        .unwrap();
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(index, &mut outcome);
    assert!(
        matches!(methods(&outcome).as_slice(), [Action::PaneClose(p)] if p.pane_id == "pane_2")
    );
}

#[test]
fn narrow_bar_reveals_focused_pane_and_cancels_a_click_when_its_slot_changes() {
    let mut projected = grouped_snapshot();
    for index in 4..=20 {
        let mut pane = projected.panes[1].clone();
        pane.pane_id = format!("pane_{index}");
        projected.panes.push(pane);
    }
    let mut state = state_with_snapshot(projected.clone());
    state.set_pane_surface(surface());
    state.compose(38, 24).unwrap();
    assert!(state.hits.tab_scroll_right.width > 0);
    let second = state.hits.tab_panes[1].0;
    state.handle_raw_events(vec![mouse(second, MouseEventKind::Down(MouseButton::Left))]);
    projected.panes.remove(1); // The old slot now shows a different pane.
    state.set_snapshot(Box::new(projected.clone()));
    state.compose(38, 24).unwrap();
    let outcome =
        state.handle_raw_events(vec![mouse(second, MouseEventKind::Up(MouseButton::Left))]);
    assert!(methods(&outcome).is_empty());

    projected.focused_pane_id = Some("pane_20".into());
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_20";
    }
    state.set_snapshot(Box::new(projected));
    let frame = state.compose(38, 24).unwrap();
    assert!(state.hits.tab_panes.iter().any(|(_, id)| id == "pane_20"));
    assert!(frame_rows(&frame)[1].contains("1.18"));
}
