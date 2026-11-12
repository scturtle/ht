use super::*;

impl ClientShellState {
    pub(super) fn active_endpoint_workspace_at(&self, point: (u16, u16)) -> Option<String> {
        self.hits
            .workspaces
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .map(|hit| hit.workspace_id.clone())
    }

    pub(super) fn endpoint_workspace_is_draggable(&self, press: &ClientWorkspacePress) -> bool {
        self.endpoint
            .snapshot
            .as_deref()
            .and_then(|snapshot| {
                snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == press.workspace_id)
            })
            .is_some_and(|workspace| {
                !workspace
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.is_linked_worktree)
            })
    }

    pub(super) fn finish_endpoint_workspace_press(
        &mut self,
        press: ClientWorkspacePress,
        outcome: &mut ClientShellInput,
    ) {
        self.focus_or_activate(
            ClientEndpointFocusTarget::Workspace(press.workspace_id),
            outcome,
        );
    }

    pub(super) fn activate_endpoint(&mut self, outcome: &mut ClientShellInput) -> bool {
        self.pending_workspace_highlight = None;
        if !self.endpoint_is_online() {
            self.receive_endpoint_unavailable("Core is not ready".into());
            outcome.repaint = true;
            return false;
        }
        true
    }
    pub(super) fn focus_or_activate(
        &mut self,
        target: ClientEndpointFocusTarget,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.activate_endpoint(outcome) {
            return false;
        }
        let method = match target {
            ClientEndpointFocusTarget::Workspace(workspace_id) => {
                crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
                    workspace_id,
                })
            }
            ClientEndpointFocusTarget::Tab(tab_id) => {
                crate::actions::Action::TabFocus(crate::actions::TabTarget { tab_id })
            }
            ClientEndpointFocusTarget::Pane(pane_id) => {
                crate::actions::Action::PaneFocus(crate::actions::PaneTarget { pane_id })
            }
        };
        self.push_endpoint_method(method, outcome);
        true
    }
}
