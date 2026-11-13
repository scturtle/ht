//! Shared workspace/tab lifecycle. Program creation and document CAS are separate.
use super::{model::invalid, session::Session};
use crate::actions;
use std::io;

impl Session {
    pub(super) fn organization_action(
        &mut self,
        method: actions::Action,
    ) -> io::Result<actions::ActionResult> {
        use actions::{Action as M, ActionResult as R};
        match method {
            M::WorkspaceCreate(p) => {
                super::organization::validate_label(p.label.as_deref())?;
                if let Some(source) = &p.source_workspace_id {
                    self.model.workspace_info(source)?;
                }
                let cwd = p.cwd.or_else(|| {
                    p.source_workspace_id
                        .as_deref()
                        .and_then(|w| self.cwd_for_workspace(w))
                });
                let record = self.run_terminal(cwd, p.env)?;
                let (workspace, id) = self.publish_creation(&record, |model| {
                    model.create_workspace(record.clone(), p.label.clone(), p.focus)
                })?;
                let tab = self
                    .model
                    .context(id)
                    .ok_or_else(|| invalid("created pane has no tab"))?
                    .1
                    .id
                    .clone();
                Ok(R::WorkspaceCreated {
                    workspace: self.model.workspace_info(&workspace)?,
                    tab: self.model.tab_info(&tab)?,
                    root_pane: self.pane_info(id)?,
                })
            }
            M::WorkspaceFocus(p) => {
                self.model.focus_workspace(&p.workspace_id)?;
                Ok(R::WorkspaceInfo {
                    workspace: self.model.workspace_info(&p.workspace_id)?,
                })
            }
            M::WorkspaceRename(p) => {
                self.change(|model| {
                    let w = model
                        .organization
                        .workspaces
                        .iter_mut()
                        .find(|w| w.id == p.workspace_id)
                        .ok_or_else(|| invalid("Core workspace not found"))?;
                    // Original `set_custom_name`: the given name is the custom name,
                    // never rewritten into a generated default.
                    w.label = p.label.clone();
                    Ok(())
                })?;
                Ok(R::WorkspaceInfo {
                    workspace: self.model.workspace_info(&p.workspace_id)?,
                })
            }
            M::WorkspaceClose(p) => {
                // Core organization has no worktree groups; either scope selects
                // this workspace, with no related workspace to close.
                let addresses = self.change(|model| model.close_workspace(&p.workspace_id))?;
                self.finish_retirements(&addresses)?;
                Ok(R::Ok {})
            }
            M::TabCreate(p) => {
                super::organization::validate_label(p.label.as_deref())?;
                let workspace = p
                    .workspace_id
                    .unwrap_or_else(|| self.model.active_workspace.clone());
                self.model.workspace_info(&workspace)?;
                let cwd = p.cwd.or_else(|| self.cwd_for_workspace(&workspace));
                let record = self.run_terminal(cwd, p.env)?;
                let (tab, id) = self.publish_creation(&record, |model| {
                    model.create_tab(&workspace, record.clone(), p.label.clone(), p.focus)
                })?;
                Ok(R::TabCreated {
                    tab: self.model.tab_info(&tab)?,
                    root_pane: self.pane_info(id)?,
                })
            }
            M::TabFocus(p) => {
                self.model.focus_tab(&p.tab_id)?;
                Ok(R::TabInfo {
                    tab: self.model.tab_info(&p.tab_id)?,
                })
            }
            M::TabRename(p) => {
                self.change(|model| {
                    let tab = model
                        .organization
                        .workspaces
                        .iter_mut()
                        .flat_map(|w| &mut w.tabs)
                        .find(|t| t.id == p.tab_id)
                        .ok_or_else(|| invalid("Core tab not found"))?;
                    tab.custom_label = Some(!p.label.is_empty());
                    tab.label = if p.label.is_empty() {
                        tab.number.to_string()
                    } else {
                        p.label.clone()
                    };
                    Ok(())
                })?;
                Ok(R::TabInfo {
                    tab: self.model.tab_info(&p.tab_id)?,
                })
            }
            M::TabMove(p) => {
                self.change(|model| {
                    let workspace = model
                        .organization
                        .workspaces
                        .iter_mut()
                        .find(|w| w.tabs.iter().any(|t| t.id == p.tab_id))
                        .ok_or_else(|| invalid("Core tab not found"))?;
                    let at = workspace
                        .tabs
                        .iter()
                        .position(|t| t.id == p.tab_id)
                        .ok_or_else(|| invalid("Core tab not found"))?;
                    let tab = workspace.tabs.remove(at);
                    workspace
                        .tabs
                        .insert(p.insert_index.min(workspace.tabs.len()), tab);
                    Ok(())
                })?;
                Ok(R::TabInfo {
                    tab: self.model.tab_info(&p.tab_id)?,
                })
            }
            M::TabClose(p) => {
                let addresses = self.change(|model| model.close_tab(&p.tab_id))?;
                self.finish_retirements(&addresses)?;
                Ok(R::Ok {})
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Core organization action is not implemented",
            )),
        }
    }
}
