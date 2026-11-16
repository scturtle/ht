use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClientGlobalMenuAction {
    Binding(keybindings::KeybindAction),
}

pub(super) fn global_menu_items() -> &'static [(&'static str, ClientGlobalMenuAction)] {
    &GLOBAL_MENU_ITEMS
}

/// The global menu content is fixed, so one static array serves painting,
/// selection, activation and the mobile menu instead of a `Vec` per call.
static GLOBAL_MENU_ITEMS: [(&str, ClientGlobalMenuAction); 3] = [
    (
        "keybinds",
        ClientGlobalMenuAction::Binding(keybindings::KeybindAction::Help),
    ),
    (
        "reload config",
        ClientGlobalMenuAction::Binding(keybindings::KeybindAction::ReloadConfig),
    ),
    (
        "detach",
        ClientGlobalMenuAction::Binding(keybindings::KeybindAction::Detach),
    ),
];

impl ClientShellState {
    pub(super) fn toggle_global_menu(&mut self) {
        if matches!(self.overlay, Some(ClientShellOverlay::GlobalMenu(_))) {
            self.overlay = None;
        } else {
            self.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
                highlighted: 0,
            }));
        }
    }

    pub(super) fn move_global_menu_selection(&mut self, delta: isize) {
        let item_count = self
            .endpoint
            .snapshot
            .as_deref()
            .map_or(0, |_| global_menu_items().len());
        let Some(ClientShellOverlay::GlobalMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_global_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(action) = self
            .endpoint
            .snapshot
            .as_deref()
            .and_then(|_| global_menu_items().get(index).map(|(_, action)| *action))
        else {
            return;
        };
        self.overlay = None;
        match action {
            ClientGlobalMenuAction::Binding(binding) => {
                self.record_binding(keybindings::KeybindMatch::Action(binding), outcome)
            }
        }
        outcome.repaint = true;
    }
}
