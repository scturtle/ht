use super::*;
use crate::actions::Action;
use crossterm::event::{MouseButton, MouseEventKind};

fn close_state(tab_count: usize) -> ClientShellState {
    let mut projected = snapshot();
    for number in 2..=tab_count {
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = format!("tab_{number}");
        tab.number = number;
        tab.focused = false;
        projected.tabs.push(tab);
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).unwrap();
    state
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn request_close(state: &mut ClientShellState, menu: bool) -> ClientShellInput {
    if menu {
        state.open_tab_context_menu("tab_1".into(), 30, 1);
        state.compose(106, 24).unwrap();
        let close_row = state.hits.context_menu_rows[2].0;
        click(state, close_row)
    } else {
        let mut outcome = ClientShellInput::default();
        state.record_binding(
            keybindings::KeybindMatch::Action(keybindings::KeybindAction::CloseTab),
            &mut outcome,
        );
        outcome
    }
}

fn assert_tab_close(outcome: &ClientShellInput) {
    let methods = outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.action),
            _ => None,
        })
        .filter(|method| !matches!(method, Action::TabFocus(_)))
        .collect::<Vec<_>>();
    assert!(matches!(methods.as_slice(), [Action::TabClose(target)] if target.tab_id == "tab_1"));
}

#[test]
fn tab_close_stays_immediate_with_one_or_multiple_tabs() {
    for tabs in [1, 2] {
        for menu in [false, true] {
            let mut state = close_state(tabs);
            assert_tab_close(&request_close(&mut state, menu));
            assert!(state.overlay.is_none());
        }
    }
}
