//! Advertise only real implementations. Replies retain the existing method shapes.
use super::{
    input,
    model::{invalid, status, MAX_PANES},
    session::Session,
    transport::DATA,
};
use crate::{actions, layout::PaneId, protocol::*};
use serde_json::{json, Value};
use std::io;

pub(super) const METHODS: &[&str] = &[
    "workspace.create",
    "workspace.focus",
    "workspace.rename",
    "workspace.close",
    "tab.create",
    "tab.focus",
    "tab.rename",
    "tab.move",
    "pane.focus",
    "pane.close",
    "pane.rename",
    "pane.selection.read",
    "pane.copy_motion",
    "pane.copy_search",
    "pane.scroll",
];

pub(super) fn advertised(hello: &Value) -> Vec<String> {
    METHODS
        .iter()
        .filter(|method| **method != "pane.rename" || super::metadata::supports_rename(hello))
        .map(|method| (*method).to_owned())
        .collect()
}

impl Session {
    pub(super) fn advertised_methods(&self) -> Vec<String> {
        advertised(&self.core.hello)
    }

    pub(super) fn cwd_for_workspace(&self, workspace: &str) -> Option<String> {
        let id = self.model.workspace_layout(workspace)?.focused();
        let record = self.model.records.get(&id)?;
        super::metadata::current_cwd(&record.facts).map(str::to_owned)
    }
    pub(super) fn run_terminal_with_labels(
        &mut self,
        cwd: Option<String>,
        mut env: std::collections::HashMap<String, String>,
        labels: Value,
    ) -> io::Result<Value> {
        if self.model.records.len() >= MAX_PANES {
            return Err(invalid("Core TUI pane limit reached"));
        }
        env.entry("TERM".into())
            .or_insert_with(|| crate::pane::PANE_TERM.into());
        env.insert("COLORTERM".into(), "truecolor".into());
        // `keep` keeps a pane usable after its program is lost: Core replaces it
        // with a shell in place instead of leaving a dead screen behind.
        self.core.request(
            "terminal.run",
            json!({"argv":self.shell,"cwd":cwd,"env":env,"no_parent":true,"keep":true,
            "labels":labels,
            "size":{"cols":self.size.cols.max(4),"rows":self.size.rows.max(2)},"display":{}}),
        )
    }
    pub(super) fn target(&self, address: Option<&str>) -> io::Result<PaneId> {
        match address {
            Some(address) => self.model.id(address),
            None => self.model.focused(),
        }
        .ok_or_else(|| invalid("Core pane not found"))
    }
    pub(super) fn pane_info(&self, id: PaneId) -> io::Result<actions::PaneInfo> {
        let record = self
            .model
            .records
            .get(&id)
            .ok_or_else(|| invalid("Core pane not found"))?;
        let facts = &record.facts;
        let view = self.views.get(&id);
        let text = |key| facts[key].as_str().map(str::to_owned);
        Ok(actions::PaneInfo {
            pane_id: record.address.clone(),
            terminal_id: record.address.clone(),
            workspace_id: self
                .model
                .context(id)
                .ok_or_else(|| invalid("pane has no workspace"))?
                .0
                .id
                .clone(),
            tab_id: self
                .model
                .context(id)
                .ok_or_else(|| invalid("pane has no tab"))?
                .1
                .id
                .clone(),
            focused: self.model.focused() == Some(id),
            cwd: text("cwd"),
            foreground_cwd: text("last_cwd"),
            restore_error: None,
            label: super::metadata::pane_label(facts),
            agent: None,
            title: None,
            terminal_title: text("title"),
            terminal_title_stripped: text("title"),
            display_agent: facts["status"]["app"].as_str().map(str::to_owned),
            agent_status: status(facts),
            state_labels: Default::default(),
            tokens: Default::default(),
            agent_session: None,
            scroll: view.and_then(|v| v.terminal.scroll_metrics()).map(|s| {
                actions::PaneScrollInfo {
                    offset_from_bottom: s.offset_from_bottom as u64,
                    max_offset_from_bottom: s.max_offset_from_bottom as u64,
                    viewport_rows: s.viewport_rows as u64,
                }
            }),
            revision: view.map_or(0, |v| v.revision),
        })
    }

    fn execute(&mut self, method: actions::Action) -> io::Result<actions::ActionResult> {
        use actions::{Action as M, ActionResult as R};
        let name = method.name();
        if !METHODS.contains(&name) {
            return Err(super::app_layout::unsupported());
        }
        if name.starts_with("workspace.") || name.starts_with("tab.") {
            return self.organization_action(method);
        }
        match method {
            M::PaneFocus(p) => {
                let id = self.target(Some(&p.pane_id))?;
                self.model.focus(id)?;
                Ok(R::PaneInfo {
                    pane: self.pane_info(id)?,
                })
            }
            M::PaneRename(p) => self.rename_pane(p),
            M::PaneClose(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let address = self.model.records[&id].address.clone();
                super::app_layout::close_terminal(&mut self.core, &address)?;
                self.model.remove(id);
                Ok(R::Ok {})
            }
            M::PaneScroll(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let view = self
                    .views
                    .get(&id)
                    .ok_or_else(|| invalid("Core view missing"))?;
                view.terminal.set_scroll_offset_from_bottom(
                    usize::try_from(p.offset_from_bottom).unwrap_or(usize::MAX),
                );
                Ok(R::PaneInfo {
                    pane: self.pane_info(id)?,
                })
            }
            M::PaneCopyMotion(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let view = self
                    .views
                    .get(&id)
                    .ok_or_else(|| invalid("Core view missing"))?;
                super::copy::motion(view, id, p)
            }
            M::PaneCopySearch(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let view = self
                    .views
                    .get(&id)
                    .ok_or_else(|| invalid("Core view missing"))?;
                super::copy::search(view, p)
            }
            M::PaneSelectionRead(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let view = self
                    .views
                    .get(&id)
                    .ok_or_else(|| invalid("Core view missing"))?;
                if p.content_revision.is_some_and(|r| r != view.revision) {
                    return Err(io::Error::other("stale_content"));
                }
                let selection = crate::selection::Selection::absolute_range(
                    id,
                    (p.anchor.row, p.anchor.col),
                    (p.cursor.row, p.cursor.col),
                );
                let text = view
                    .terminal
                    .extract_selection(&selection)
                    .ok_or_else(|| invalid("selection unavailable"))?;
                Ok(R::PaneSelection {
                    pane_id: p.pane_id,
                    text,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Action is not available with the Core backend",
            )),
        }
    }

    pub fn message(&mut self, message: ClientMessage) -> io::Result<Vec<ServerMessage>> {
        let mut out = Vec::new();
        match message {
            ClientMessage::ClientShellResize {
                cell_width_px,
                cell_height_px,
                surface_size,
                ..
            } => {
                self.resize(surface_size, cell_width_px, cell_height_px)?;
            }
            ClientMessage::ClientShellFocus { focused } => {
                self.focused = focused;
                self.focus()?;
            }
            ClientMessage::ClientShellHostTheme { update } => {
                self.theme(update)?;
            }
            ClientMessage::ClientShellPaneInput { pane_id, events } => {
                let result = (|| -> io::Result<()> {
                    if let Some((channel, data)) =
                        input::for_target(&self.model, &self.views, &pane_id, &events)?
                    {
                        self.core.send(channel, DATA, &data)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    out.push(input::rejection(&events, &error.to_string()));
                }
                self.dirty = true;
            }
            ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                let request = *request;
                let focus_change = matches!(
                    &request.action,
                    actions::Action::PaneClose(_)
                        | actions::Action::PaneFocus(_)
                        | actions::Action::WorkspaceCreate(_)
                        | actions::Action::WorkspaceClose(_)
                        | actions::Action::WorkspaceFocus(_)
                        | actions::Action::WorkspaceRename(_)
                        | actions::Action::TabCreate(_)
                        | actions::Action::TabFocus(_)
                        | actions::Action::TabRename(_)
                        | actions::Action::TabMove(_)
                );
                let name = request.action.name();
                let read_only = name.ends_with(".get")
                    || name.ends_with(".list")
                    || matches!(
                        &request.action,
                        actions::Action::PaneSelectionRead(_)
                            | actions::Action::PaneCopyMotion(_)
                            | actions::Action::PaneCopySearch(_)
                    );
                let metadata_only = matches!(&request.action, actions::Action::PaneRename(_));
                let before = self.model.revision;
                let mut result = if boot_id != self.model.boot {
                    Err(invalid("Core client generation changed"))
                } else {
                    self.execute(request.action)
                };
                if focus_change && self.model.revision != before {
                    self.snapshot_dirty = true;
                    if let Err(error) = self
                        .sync_views()
                        .and_then(|()| self.focus())
                        .and_then(|()| self.save())
                    {
                        result = Err(io::Error::new(error.kind(), format!("layout/focus changed, but presentation synchronization failed: {error}")));
                    }
                }
                let request_id = request.id.clone();
                let response = match result {
                    Ok(result) => {
                        self.dirty |= !read_only && !metadata_only;
                        Ok(Box::new(result))
                    }
                    Err(error) => {
                        let code = if error.to_string() == "stale_content" {
                            "stale_content"
                        } else if error.to_string() == "query_too_large" {
                            "query_too_large"
                        } else if error.to_string() == "copy_motion_unavailable" {
                            "copy_motion_unavailable"
                        } else if error.kind() == io::ErrorKind::Unsupported {
                            "unsupported_method"
                        } else {
                            "core_action_failed"
                        };
                        Err(EndpointResponseError {
                            code: Some(code.to_owned()),
                            message: error.to_string(),
                        })
                    }
                };
                out.push(ServerMessage::ClientShellEndpointResponse {
                    boot_id,
                    request_id,
                    result: response,
                });
            }
            // These are client presentation preferences, not runtime commands.
            ClientMessage::ClientShellMouseCapture { .. } => {}
            other => {
                tracing::debug!(?other, "unimplemented non-runtime Core client message");
            }
        }
        Ok(out)
    }
}
