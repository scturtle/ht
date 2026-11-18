use super::*;

impl ClientContextMenuOverlay {
    pub(super) fn items(&self) -> Vec<ClientContextMenuItem> {
        use ClientContextMenuAction as Action;

        let item = |label, action| ClientContextMenuItem { label, action };
        let mut items = match &self.target {
            ClientContextMenuTarget::Workspace { close_group, .. } => vec![
                item("Rename", Action::Rename),
                item(
                    if *close_group { "Close group" } else { "Close" },
                    Action::Close,
                ),
            ],
            ClientContextMenuTarget::Tab { .. } => vec![
                item("New tab", Action::NewTab),
                item("Rename", Action::Rename),
            ],
            ClientContextMenuTarget::Pane {
                has_manual_label,
                right_click_passthrough,
                tab_group,
                ..
            } => {
                let mut items = vec![item("Rename pane", Action::RenamePane)];
                if *has_manual_label {
                    items.push(item("Clear pane name", Action::ClearPaneName));
                }
                items.extend([
                    item(
                        if *right_click_passthrough {
                            "Use Herdr right-click menu"
                        } else {
                            "Send right-clicks to pane"
                        },
                        Action::ToggleRightClickPassthrough,
                    ),
                    item("Close pane", Action::ClosePane),
                ]);
                if tab_group.is_some() {
                    items.push(item("Rename tab", Action::RenameTab));
                }
                items
            }
        };
        items.retain(|item| {
            let method = match item.action {
                Action::RenamePane | Action::ClearPaneName => "pane.rename",
                Action::ClosePane => "pane.close",
                Action::RenameTab => "tab.rename",
                Action::NewTab => "tab.create",
                Action::Rename => match self.target {
                    ClientContextMenuTarget::Workspace { .. } => "workspace.rename",
                    _ => "tab.rename",
                },
                Action::Close => "workspace.close",
                Action::ToggleRightClickPassthrough => return true,
            };
            self.methods
                .as_ref()
                .is_none_or(|methods| methods.contains(method))
        });
        items
    }
}

impl ClientShellState {
    /// Re-gate an already open context menu against the current method set.
    ///
    /// The highlight follows its action rather than its index: an item that the
    /// new set does not support cannot leave a different action sitting under
    /// the old position for the next Enter.
    pub(super) fn refresh_context_menu_methods(&mut self) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_ref() else {
            return;
        };
        let highlighted = menu.highlighted;
        let selected = menu.items().get(highlighted).map(|item| item.action);
        let methods = self.endpoint.methods.clone();
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        menu.methods = methods;
        let items = menu.items();
        menu.highlighted = selected
            .and_then(|action| items.iter().position(|item| item.action == action))
            // No valid selection if the action disappeared. The existing
            // out-of-range activation guard consumes Enter and closes the menu;
            // do not substitute another action or send Enter to the program.
            .unwrap_or(items.len());
        // Rows captured against the old item list would activate whatever moved
        // under them: drop them until the next compose re-lays the menu out.
        self.hits.context_menu_rows.clear();
    }

    pub(super) fn open_workspace_context_menu(&mut self, workspace_id: String, x: u16, y: u16) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
        else {
            return;
        };
        let close_group = super::navigation_rows::workspace_close_is_group(snapshot, workspace);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            methods: self.endpoint.methods.clone(),
            target: ClientContextMenuTarget::Workspace {
                workspace_id,
                close_group,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn open_tab_context_menu(&mut self, tab_id: String, x: u16, y: u16) {
        let Some(tab) = self
            .endpoint
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id))
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            methods: self.endpoint.methods.clone(),
            target: ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id: tab.workspace_id.clone(),
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn open_pane_context_menu(&mut self, pane_id: String, x: u16, y: u16) {
        self.open_pane_menu(pane_id, false, x, y);
    }

    pub(super) fn open_tab_pane_context_menu(&mut self, pane_id: String, x: u16, y: u16) {
        self.open_pane_menu(pane_id, true, x, y);
    }

    fn open_pane_menu(&mut self, pane_id: String, from_tab_bar: bool, x: u16, y: u16) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            methods: self.endpoint.methods.clone(),
            target: ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id: pane.workspace_id.clone(),
                tab_group: from_tab_bar.then(|| pane.tab_id.clone()),
                has_manual_label: pane.label.is_some(),
                right_click_passthrough: pane.right_click_passthrough,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn move_context_menu_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        let item_count = menu.items().len();
        if item_count == 0 {
            return;
        }
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_context_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.take() else {
            return;
        };
        let Some(action) = menu.items().get(index).map(|item| item.action) else {
            outcome.repaint = true;
            return;
        };
        match menu.target {
            ClientContextMenuTarget::Workspace {
                workspace_id,
                close_group,
                ..
            } => self.activate_workspace_context_action(workspace_id, close_group, action, outcome),
            ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id,
            } => self.activate_tab_context_action(tab_id, workspace_id, action, outcome),
            ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id,
                right_click_passthrough,
                tab_group,
                ..
            } => {
                if action == ClientContextMenuAction::RenameTab {
                    if let Some(tab_id) = tab_group.filter(|tab_id| {
                        self.endpoint.snapshot.as_deref().is_some_and(|snapshot| {
                            snapshot
                                .panes
                                .iter()
                                .any(|pane| pane.pane_id == pane_id && &pane.tab_id == tab_id)
                        })
                    }) {
                        self.activate_tab_context_action(
                            tab_id,
                            workspace_id,
                            ClientContextMenuAction::Rename,
                            outcome,
                        );
                    }
                } else {
                    self.activate_pane_context_action(
                        pane_id,
                        right_click_passthrough,
                        action,
                        outcome,
                    );
                }
            }
        }
        outcome.repaint = true;
    }

    fn activate_workspace_context_action(
        &mut self,
        workspace_id: String,
        close_group: bool,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        match action {
            ClientContextMenuAction::Rename => {
                let label = self
                    .endpoint
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| {
                        snapshot
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.workspace_id == workspace_id)
                    })
                    .map(|workspace| workspace.label.clone());
                if let Some(label) = label {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename workspace",
                        input: TextEditor::new(&label, false),
                        target: ClientRenameTarget::Workspace { workspace_id },
                    }));
                }
            }
            ClientContextMenuAction::Close => {
                self.request_workspace_close(workspace_id, Some(close_group), outcome);
            }
            _ => {}
        }
    }

    fn activate_tab_context_action(
        &mut self,
        tab_id: String,
        workspace_id: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::actions::{Action, TabTarget};

        self.push_endpoint_method(
            Action::TabFocus(TabTarget {
                tab_id: tab_id.clone(),
            }),
            outcome,
        );
        match action {
            ClientContextMenuAction::NewTab => {
                self.push_endpoint_method(
                    Action::TabCreate(crate::actions::TabCreateParams {
                        workspace_id: Some(workspace_id),
                        cwd: None,
                        focus: true,
                        label: None,
                        env: Default::default(),
                    }),
                    outcome,
                );
            }
            ClientContextMenuAction::Rename => {
                let tab = self
                    .endpoint
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id));
                if let Some(tab) = tab {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename tab",
                        input: TextEditor::new(&tab.label, false),
                        target: ClientRenameTarget::Tab {
                            tab_id,
                            auto_name: !tab.custom_label,
                            original_name: tab.label.clone(),
                        },
                    }));
                }
            }
            _ => {}
        }
    }

    fn activate_pane_context_action(
        &mut self,
        pane_id: String,
        right_click_passthrough: bool,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::actions::{Action, PaneInputSetParams, PaneRenameParams, PaneRightClickTarget};

        match action {
            ClientContextMenuAction::RenamePane => {
                let label = self.endpoint.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .panes
                        .iter()
                        .find(|pane| pane.pane_id == pane_id)
                        .and_then(|pane| pane.label.clone())
                });
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename pane",
                    input: TextEditor::new(label.as_deref().unwrap_or_default(), label.is_none()),
                    target: ClientRenameTarget::Pane { pane_id },
                }));
            }
            ClientContextMenuAction::ClearPaneName => self.push_endpoint_method(
                Action::PaneRename(PaneRenameParams {
                    pane_id,
                    label: None,
                }),
                outcome,
            ),
            ClientContextMenuAction::ToggleRightClickPassthrough => self.push_endpoint_method(
                Action::PaneInputSet(PaneInputSetParams {
                    pane_id,
                    right_click: if right_click_passthrough {
                        PaneRightClickTarget::Herdr
                    } else {
                        PaneRightClickTarget::Pane
                    },
                }),
                outcome,
            ),
            ClientContextMenuAction::ClosePane => self.request_pane_close(pane_id, outcome),
            _ => {}
        }
    }
}
