use super::*;

impl ClientShellState {
    pub(super) fn open_navigator_overlay(&mut self) {
        let mut navigator = ClientNavigatorOverlay::default();
        let rows = super::navigator::navigator_rows(&self.endpoint);
        navigator.selected = rows
            .iter()
            .find(|row| row.current)
            .map(|row| row.target.clone());
        self.overlay = Some(ClientShellOverlay::Navigator(navigator));
    }

    pub(super) fn move_navigator_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let rows = super::navigator::navigator_rows(&self.endpoint);
        if rows.is_empty() {
            navigator.selected = None;
            return;
        }
        let selected = super::navigator::navigator_selected_index(&rows, navigator).unwrap_or(0);
        let next =
            (selected as isize + delta).clamp(0, rows.len().saturating_sub(1) as isize) as usize;
        navigator.selected = Some(rows[next].target.clone());
    }

    pub(super) fn scroll_navigator_to(&mut self, scroll: usize, viewport_rows: usize) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let rows = super::navigator::navigator_rows(&self.endpoint);
        let viewport_rows = viewport_rows.max(1);
        navigator.scroll = scroll.min(rows.len().saturating_sub(viewport_rows));
        let selected = super::navigator::navigator_selected_index(&rows, navigator).unwrap_or(0);
        // Keep the selection in the dragged viewport so rendering does not snap back to it.
        let selected = selected.clamp(navigator.scroll, navigator.scroll + viewport_rows - 1);
        navigator.selected = rows.get(selected).map(|row| row.target.clone());
    }

    fn move_navigator_workspace(&mut self, forward: bool) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let rows = super::navigator::navigator_rows(&self.endpoint);
        let Some(selected) = super::navigator::navigator_selected_index(&rows, navigator) else {
            return;
        };
        let section = rows[..=selected]
            .iter()
            .rposition(|row| !matches!(row.target, ClientNavigatorTarget::Pane { .. }))
            .unwrap_or(selected);
        let mut destinations = rows.windows(2).enumerate().filter(|(index, pair)| {
            matches!(pair[0].target, ClientNavigatorTarget::Workspace { .. })
                && matches!(pair[1].target, ClientNavigatorTarget::Pane { .. })
                && if forward {
                    *index > section
                } else {
                    *index < section
                }
        });
        let destination = if forward {
            destinations.next()
        } else {
            destinations.next_back()
        };
        if let Some((_, pair)) = destination {
            navigator.selected = Some(pair[1].target.clone());
        }
    }

    pub(super) fn accept_navigator_selection(&mut self, outcome: &mut ClientShellInput) {
        let target = self.overlay.as_ref().and_then(|overlay| match overlay {
            ClientShellOverlay::Navigator(navigator) => {
                let rows = super::navigator::navigator_rows(&self.endpoint);
                super::navigator::selected_navigator_target(&rows, navigator)
            }
            _ => None,
        });
        let Some(target) = target else {
            return;
        };
        // Offline connections keep the chooser open. Capability refusals keep
        // the existing dismissal policy, but admission never enqueues them.
        let online = self.endpoint_is_online();
        let method = match target {
            ClientNavigatorTarget::Workspace { workspace_id } => {
                crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
                    workspace_id,
                })
            }
            ClientNavigatorTarget::Pane { pane_id } => {
                crate::actions::Action::PaneFocus(crate::actions::PaneTarget { pane_id })
            }
        };
        self.push_endpoint_method(method, outcome);
        if online {
            self.overlay = None;
        }
        outcome.repaint = true;
    }

    pub(super) fn workspace_action_id(&self) -> Option<String> {
        self.navigate_workspace_id
            .as_ref()
            .filter(|target| self.navigation_target_valid(target))
            .map(|target| target.workspace_id.clone())
            .or_else(|| {
                self.endpoint
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.focused_workspace_id.clone())
            })
    }

    pub(super) fn open_rename_workspace_overlay(&mut self) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(workspace_id) = self.workspace_action_id() else {
            return;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            input: TextEditor::new(&workspace.label, false),
            target: ClientRenameTarget::Workspace { workspace_id },
        }));
    }

    pub(super) fn open_rename_tab_overlay(&mut self) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(tab_id) = snapshot.focused_tab_id.as_deref() else {
            return;
        };
        let Some(tab) = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id) else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename tab",
            input: TextEditor::new(&tab.label, false),
            target: ClientRenameTarget::Tab {
                tab_id: tab.tab_id.clone(),
                auto_name: !tab.custom_label,
                original_name: tab.label.clone(),
            },
        }));
    }

    pub(super) fn open_rename_pane_overlay(&mut self) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(pane_id) = snapshot.focused_pane_id.as_deref() else {
            return;
        };
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename pane",
            input: TextEditor::new(
                pane.label.as_deref().unwrap_or_default(),
                pane.label.is_none(),
            ),
            target: ClientRenameTarget::Pane {
                pane_id: pane.pane_id.clone(),
            },
        }));
    }

    pub(super) fn insert_overlay_text(&mut self, text: &str) -> bool {
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::Rename(rename)) => {
                rename.input.insert(text);
                true
            }
            Some(ClientShellOverlay::Help(help)) if help.search_focused => {
                if help.query.insert(text) {
                    help.scroll = 0;
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn route_overlay_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        use crossterm::event::KeyModifiers;

        if matches!(self.overlay, Some(ClientShellOverlay::GlobalMenu(_))) {
            match key.code {
                KeyCode::Esc => {
                    self.overlay = None;
                    outcome.repaint = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_global_menu_selection(-1);
                    outcome.repaint = true;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_global_menu_selection(1);
                    outcome.repaint = true;
                }
                KeyCode::Enter => {
                    let highlighted = match self.overlay.as_ref() {
                        Some(ClientShellOverlay::GlobalMenu(menu)) => menu.highlighted,
                        _ => return,
                    };
                    self.activate_global_menu_item(highlighted, outcome);
                }
                _ => {}
            }
            return;
        }

        if matches!(self.overlay, Some(ClientShellOverlay::ContextMenu(_))) {
            match key.code {
                KeyCode::Esc => {
                    self.overlay = None;
                    outcome.repaint = true;
                }
                KeyCode::Up => {
                    self.move_context_menu_selection(-1);
                    outcome.repaint = true;
                }
                KeyCode::Down => {
                    self.move_context_menu_selection(1);
                    outcome.repaint = true;
                }
                KeyCode::Enter => {
                    let highlighted = match self.overlay.as_ref() {
                        Some(ClientShellOverlay::ContextMenu(menu)) => menu.highlighted,
                        _ => return,
                    };
                    self.activate_context_menu_item(highlighted, outcome);
                }
                _ => {}
            }
            return;
        }

        if matches!(self.overlay, Some(ClientShellOverlay::Navigator(_))) {
            let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
            if code == KeyCode::Esc {
                self.overlay = None;
                outcome.repaint = true;
                return;
            }
            if code == KeyCode::Enter {
                self.accept_navigator_selection(outcome);
                return;
            }
            if matches!(code, KeyCode::Left | KeyCode::Right) && modifiers.is_empty() {
                self.move_navigator_workspace(code == KeyCode::Right);
                outcome.repaint = true;
                return;
            }
            if code == KeyCode::Home && modifiers.is_empty() {
                if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                    navigator.selected = None;
                    navigator.scroll = 0;
                }
                outcome.repaint = true;
                return;
            }
            if matches!(code, KeyCode::End | KeyCode::Char('G')) && modifiers.is_empty() {
                let last = self.overlay.as_ref().and_then(|overlay| match overlay {
                    ClientShellOverlay::Navigator(_) => {
                        super::navigator::navigator_rows(&self.endpoint)
                            .last()
                            .map(|row| row.target.clone())
                    }
                    _ => None,
                });
                if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                    navigator.selected = last;
                }
                outcome.repaint = true;
                return;
            }
            if matches!(code, KeyCode::Down | KeyCode::Char('j')) && modifiers.is_empty() {
                self.move_navigator_selection(1);
                outcome.repaint = true;
                return;
            }
            if matches!(code, KeyCode::Up | KeyCode::Char('k')) && modifiers.is_empty() {
                self.move_navigator_selection(-1);
                outcome.repaint = true;
                return;
            }
            if code == KeyCode::Char('d') && modifiers.contains(KeyModifiers::CONTROL) {
                self.move_navigator_selection(8);
                outcome.repaint = true;
                return;
            }
            if code == KeyCode::Char('u') && modifiers.contains(KeyModifiers::CONTROL) {
                self.move_navigator_selection(-8);
                outcome.repaint = true;
                return;
            }
            return;
        }

        if matches!(self.overlay, Some(ClientShellOverlay::Help(_))) {
            let text_character = crate::input::keybind_help_text_char(key);
            let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
            let search_focused = matches!(
                self.overlay,
                Some(ClientShellOverlay::Help(ClientHelpOverlay {
                    search_focused: true,
                    ..
                }))
            );
            if search_focused {
                if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                    if let Some(content_changed) = help.query.handle_key(key) {
                        if content_changed {
                            help.scroll = 0;
                        }
                        outcome.repaint = true;
                        return;
                    }
                }
                match code {
                    KeyCode::Esc => {
                        if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                            help.search_focused = false;
                            help.query.clear();
                            help.scroll = 0;
                        }
                    }
                    KeyCode::Enter => self.overlay = None,
                    KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Char('n' | 'p')
                        if !matches!(code, KeyCode::Char(_))
                            || modifiers == KeyModifiers::CONTROL =>
                    {
                        let delta = match code {
                            KeyCode::Up | KeyCode::Char('p') => -1,
                            KeyCode::Down | KeyCode::Char('n') => 1,
                            KeyCode::PageUp => -8,
                            KeyCode::PageDown => 8,
                            _ => unreachable!(),
                        };
                        if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                            help.scroll = help
                                .scroll
                                .saturating_add_signed(delta)
                                .min(self.hits.help_max_scroll);
                        }
                    }
                    _ => {}
                }
                outcome.repaint = true;
                return;
            }

            match code {
                KeyCode::Esc | KeyCode::Enter => self.overlay = None,
                KeyCode::Home => {
                    if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                        help.scroll = 0;
                    }
                }
                KeyCode::End => {
                    if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                        help.scroll = self.hits.help_max_scroll;
                    }
                }
                KeyCode::Up
                | KeyCode::Char('k')
                | KeyCode::Down
                | KeyCode::Char('j')
                | KeyCode::PageUp
                | KeyCode::PageDown => {
                    let delta = match code {
                        KeyCode::Up | KeyCode::Char('k') => -1,
                        KeyCode::Down | KeyCode::Char('j') => 1,
                        KeyCode::PageUp => -8,
                        KeyCode::PageDown => 8,
                        _ => unreachable!(),
                    };
                    if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                        help.scroll = help
                            .scroll
                            .saturating_add_signed(delta)
                            .min(self.hits.help_max_scroll);
                    }
                }
                _ if text_character == Some('/') => {
                    if let Some(ClientShellOverlay::Help(help)) = self.overlay.as_mut() {
                        help.search_focused = true;
                        help.scroll = 0;
                    }
                }
                _ if text_character == Some('?') => self.overlay = None,
                _ => {}
            }
            outcome.repaint = true;
            return;
        }

        let Some(ClientShellOverlay::Rename(rename)) = self.overlay.as_mut() else {
            return;
        };
        if key.code == KeyCode::Enter {
            self.save_rename_overlay(outcome);
            return;
        }
        if key.code == KeyCode::Esc {
            self.overlay = None;
            outcome.repaint = true;
            return;
        }
        if key
            .generated_text
            .as_deref()
            .is_some_and(|text| !text.is_empty())
        {
            outcome.repaint |= rename.input.handle_key(key).is_some();
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            rename.input.clear();
            outcome.repaint = true;
            return;
        }
        if key.code == KeyCode::Backspace && key.modifiers.contains(KeyModifiers::SUPER) {
            rename.input.clear();
            outcome.repaint = true;
            return;
        }
        if rename.input.handle_key(key).is_some() {
            outcome.repaint = true;
        }
    }

    pub(super) fn save_rename_overlay(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Rename(rename)) = self.overlay.take() else {
            return;
        };
        let trimmed = rename.input.trim();
        let method = match rename.target {
            ClientRenameTarget::Workspace { workspace_id } => (!trimmed.is_empty()).then(|| {
                crate::actions::Action::WorkspaceRename(crate::actions::WorkspaceRenameParams {
                    workspace_id,
                    label: trimmed.to_owned(),
                })
            }),
            ClientRenameTarget::Tab {
                tab_id,
                auto_name,
                original_name,
            } => (!(trimmed.is_empty() || auto_name && trimmed == original_name)).then(|| {
                crate::actions::Action::TabRename(crate::actions::TabRenameParams {
                    tab_id,
                    label: trimmed.to_owned(),
                })
            }),
            ClientRenameTarget::Pane { pane_id } => Some(crate::actions::Action::PaneRename(
                crate::actions::PaneRenameParams {
                    pane_id,
                    label: Some(trimmed.to_owned()),
                },
            )),
        };
        if let Some(method) = method {
            self.push_endpoint_method(method, outcome);
        }
        outcome.repaint = true;
    }

    pub(super) fn request_workspace_close(
        &mut self,
        workspace_id: String,
        close_group: Option<bool>,
        outcome: &mut ClientShellInput,
    ) {
        let close_group = close_group.unwrap_or_else(|| {
            self.endpoint.snapshot.as_deref().is_some_and(|snapshot| {
                snapshot.workspaces.iter().any(|workspace| {
                    workspace.workspace_id == workspace_id
                        && super::navigation_rows::workspace_close_is_group(snapshot, workspace)
                })
            })
        });
        self.push_endpoint_method(
            crate::actions::Action::WorkspaceClose(crate::actions::WorkspaceCloseParams {
                workspace_id,
                close_group,
            }),
            outcome,
        );
    }

    pub(super) fn request_pane_close(&mut self, pane_id: String, outcome: &mut ClientShellInput) {
        self.push_endpoint_method(
            crate::actions::Action::PaneClose(crate::actions::PaneTarget { pane_id }),
            outcome,
        );
    }

    pub(super) fn request_tab_close(&mut self, tab_id: String, outcome: &mut ClientShellInput) {
        self.push_endpoint_method(
            crate::actions::Action::TabClose(crate::actions::TabTarget { tab_id }),
            outcome,
        );
    }
}
