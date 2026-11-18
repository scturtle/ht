//! App workspace/tab actions. Focus is local, edits go through document CAS.
use super::session::Session;
use crate::actions;
use std::io;

impl Session {
    pub(super) fn organization_action(
        &mut self,
        method: actions::Action,
    ) -> io::Result<actions::ActionResult> {
        use actions::{Action as M, ActionResult as R};
        match method {
            M::WorkspaceCreate(p) => self.create_app_workspace(p),
            M::WorkspaceFocus(p) => {
                self.model.focus_workspace(&p.workspace_id)?;
                Ok(R::WorkspaceInfo {
                    workspace: self.model.workspace_info(&p.workspace_id)?,
                })
            }
            M::WorkspaceRename(p) => self.rename_app_workspace(p),
            M::WorkspaceClose(p) => self.close_app_workspace(p),
            M::TabCreate(p) => self.create_app_tab(p),
            M::TabFocus(p) => {
                self.model.focus_tab(&p.tab_id)?;
                Ok(R::TabInfo {
                    tab: self.model.tab_info(&p.tab_id)?,
                })
            }
            M::TabRename(p) => self.rename_app_tab(p),
            M::TabMove(p) => self.move_app_tab(p),
            _ => Err(super::app_layout::unsupported()),
        }
    }
}
