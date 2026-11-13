//! Advertise only real implementations. Replies retain the existing method shapes.
use super::{
    input,
    model::{invalid, status, Model, MAX_PANES},
    session::Session,
    transport::DATA,
};
use crate::{
    actions,
    layout::{NavDirection, Node, PaneId, TileLayout},
    protocol::*,
};
use ratatui::layout::Direction;
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
    "tab.close",
    "pane.focus",
    "pane.zoom",
    "pane.swap",
    "pane.focus_direction",
    "pane.split",
    "pane.close",
    "pane.rename",
    "pane.resize",
    "layout.set_split_ratio",
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

pub(super) fn nav(direction: actions::PaneDirection) -> NavDirection {
    match direction {
        actions::PaneDirection::Left => NavDirection::Left,
        actions::PaneDirection::Right => NavDirection::Right,
        actions::PaneDirection::Up => NavDirection::Up,
        actions::PaneDirection::Down => NavDirection::Down,
    }
}

type SplitBoundary = (Direction, Vec<PaneId>, Vec<PaneId>);
fn split_members(layout: &TileLayout, path: &[bool]) -> Option<SplitBoundary> {
    fn collect(node: &Node, ids: &mut Vec<PaneId>) {
        match node {
            Node::Pane(id) => ids.push(*id),
            Node::Split { first, second, .. } => {
                collect(first, ids);
                collect(second, ids);
            }
        }
    }
    let mut node = layout.root();
    for branch in path {
        let Node::Split { first, second, .. } = node else {
            return None;
        };
        node = if *branch { second } else { first };
    }
    let Node::Split {
        direction,
        first,
        second,
        ..
    } = node
    else {
        return None;
    };
    let (mut left, mut right) = (Vec::new(), Vec::new());
    collect(first, &mut left);
    collect(second, &mut right);
    Some((*direction, left, right))
}

impl Session {
    pub(super) fn cwd_for_workspace(&self, workspace: &str) -> Option<String> {
        let tab = self.model.active_tabs.get(workspace)?;
        let id = self.model.layouts.get(tab)?.focused();
        let record = self.model.records.get(&id)?;
        record.facts["last_cwd"]
            .as_str()
            .or(record.facts["cwd"].as_str())
            .map(str::to_owned)
    }
    pub(super) fn run_terminal(
        &mut self,
        cwd: Option<String>,
        mut env: std::collections::HashMap<String, String>,
    ) -> io::Result<Value> {
        if self.model.records.len() >= MAX_PANES {
            return Err(invalid("Core TUI pane limit reached"));
        }
        let cwd = cwd.or_else(|| self.cwd_for_workspace(&self.model.active_workspace));
        env.insert("TERM".into(), crate::pane::PANE_TERM.into());
        env.insert("COLORTERM".into(), "truecolor".into());
        // `keep` keeps a pane usable after its program is lost: Core replaces it
        // with a shell in place instead of leaving a dead screen behind.
        self.core.request("terminal.run", json!({"argv":self.shell,"cwd":cwd,"env":env,"no_parent":true,"keep":true,
            "labels":{"herdr":"tui","herdr.operation":format!("{}-{}",std::process::id(),self.model.revision)},
            "size":{"cols":self.size.cols.max(4),"rows":self.size.rows.max(2)},"display":{}}))
    }
    pub(super) fn publish_creation<T>(
        &mut self,
        record: &Value,
        apply: impl FnMut(&mut Model) -> io::Result<T>,
    ) -> io::Result<T> {
        self.change(apply).map_err(|error| io::Error::new(error.kind(), format!("publication not confirmed for Core program {}; kept alive for reconciliation: {error}", record["address"])))
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
        if name.starts_with("workspace.") || name.starts_with("tab.") {
            return self.organization_action(method);
        }
        match method {
            M::PaneSwap(p) => {
                let p = super::swap::pin_source(&self.model, p)?;
                let area = self.area();
                let mut swap = self.change(|model| model.swap_panes(&p, area))?;
                swap.layout = super::view::api_layout_snapshot(
                    &self.model,
                    &swap.layout.tab_id,
                    area,
                    &self.ui,
                )?;
                Ok(R::PaneSwap { swap })
            }
            M::PaneZoom(p) => {
                let id = self.target(p.pane_id.as_deref())?;
                let outcome = self.model.zoom_pane(id, p.mode)?;
                let tab = self
                    .model
                    .active_tab()
                    .ok_or_else(|| invalid("no active tab"))?;
                let layout =
                    super::view::api_layout_snapshot(&self.model, tab, self.area(), &self.ui)?;
                Ok(R::PaneZoom {
                    zoom: actions::PaneZoomResult {
                        changed: outcome.changed || outcome.focus_changed,
                        zoom_changed: outcome.changed,
                        focus_changed: outcome.focus_changed,
                        reason: outcome.reason,
                        pane_id: self.model.records[&id].address.clone(),
                        focused_pane_id: layout.focused_pane_id.clone(),
                        zoomed: outcome.zoomed,
                        layout,
                    },
                })
            }
            M::PaneFocus(p) => {
                let id = self.target(Some(&p.pane_id))?;
                self.model.focus(id)?;
                Ok(R::PaneInfo {
                    pane: self.pane_info(id)?,
                })
            }
            M::PaneSplit(p) => {
                if let Some(workspace) = &p.workspace_id {
                    self.model.workspace_info(workspace)?;
                }
                if p.right_click != actions::PaneRightClickTarget::Herdr {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Core right-click passthrough is not implemented",
                    ));
                }
                let target = if p.target_pane_id.is_none() {
                    p.workspace_id
                        .as_ref()
                        .and_then(|w| self.model.active_tabs.get(w))
                        .and_then(|tab| self.model.layouts.get(tab))
                        .map(TileLayout::focused)
                } else {
                    None
                }
                .map(Ok)
                .unwrap_or_else(|| self.target(p.target_pane_id.as_deref()))?;
                if p.workspace_id.as_ref().is_some_and(|w| {
                    self.model
                        .context(target)
                        .is_none_or(|(workspace, _)| &workspace.id != w)
                }) {
                    return Err(invalid("split target belongs to a different workspace"));
                }
                if self.model.records.len() >= MAX_PANES {
                    return Err(invalid("Core TUI pane limit reached"));
                }
                let ratio = p.ratio.unwrap_or(0.5);
                if !ratio.is_finite() || !(0.05..=0.95).contains(&ratio) {
                    return Err(invalid("invalid Core split ratio"));
                }
                let cwd = p.cwd.or_else(|| {
                    self.model.records[&target].facts["last_cwd"]
                        .as_str()
                        .or(self.model.records[&target].facts["cwd"].as_str())
                        .map(str::to_owned)
                });
                let result = self.run_terminal(cwd, p.env)?;
                let direction = if p.direction == actions::SplitDirection::Right {
                    Direction::Horizontal
                } else {
                    Direction::Vertical
                };
                let id = self.publish_creation(&result, |model| {
                    model.insert_at(result.clone(), Some(target), direction, ratio, p.focus)
                })?;
                Ok(R::PaneInfo {
                    pane: self.pane_info(id)?,
                })
            }
            M::PaneRename(p) => self.rename_pane(p),
            M::PaneClose(p) => {
                let id = self.target(Some(&p.pane_id))?;
                let address = self.model.records[&id].address.clone();
                self.change(|model| model.retire(id))?;
                self.finish_retirements(&[address])?;
                Ok(R::Ok {})
            }
            M::PaneFocusDirection(p) => {
                let source = self.target(p.pane_id.as_deref())?;
                let tab = self
                    .model
                    .context(source)
                    .ok_or_else(|| invalid("Core pane has no tab"))?
                    .1
                    .id
                    .clone();
                let layout = self
                    .model
                    .layouts
                    .get(&tab)
                    .ok_or_else(|| invalid("Core layout missing"))?;
                let panes = layout.panes(self.area());
                let source_info = panes
                    .iter()
                    .find(|pane| pane.id == source)
                    .ok_or_else(|| invalid("Core focus source missing"))?;
                let neighbor =
                    crate::layout::find_in_direction(source_info, nav(p.direction), &panes);
                if let Some(id) = neighbor {
                    self.model.focus(id)?;
                }
                Ok(R::PaneFocusDirection {
                    focus: actions::PaneFocusDirectionResult {
                        changed: neighbor.is_some(),
                        reason: neighbor
                            .is_none()
                            .then_some(actions::PaneFocusDirectionReason::NoNeighbor),
                        source_pane_id: self.model.records[&source].address.clone(),
                        focused_pane_id: self
                            .model
                            .focused()
                            .map(|id| self.model.records[&id].address.clone()),
                        layout: super::view::api_layout_snapshot(
                            &self.model,
                            &tab,
                            self.area(),
                            &self.ui,
                        )?,
                    },
                })
            }
            M::PaneResize(p) => {
                let id = self.target(p.pane_id.as_deref())?;
                let amount = p.amount.unwrap_or(0.05);
                if !amount.is_finite() || amount <= 0.0 || amount > 1.0 {
                    return Err(invalid("invalid Core resize amount"));
                }
                let area = self.area();
                let tab = self
                    .model
                    .context(id)
                    .ok_or_else(|| invalid("Core pane has no tab"))?
                    .1
                    .id
                    .clone();
                let changed = self.change(|model| {
                    let layout = model
                        .layouts
                        .get_mut(&tab)
                        .ok_or_else(|| invalid("Core layout missing"))?;
                    if !layout.pane_ids().contains(&id) {
                        return Err(invalid("Core resize target disappeared"));
                    }
                    Ok(layout.resize_pane(id, nav(p.direction), amount, area))
                })?;
                Ok(R::PaneResize {
                    resize: actions::PaneResizeResult {
                        changed,
                        reason: (!changed).then_some(actions::PaneResizeReason::Unchanged),
                        pane_id: self.model.records[&id].address.clone(),
                        focused_pane_id: self.model.records
                            [&self.model.focused().ok_or_else(|| invalid("no focus"))?]
                            .address
                            .clone(),
                        layout: super::view::api_layout_snapshot(
                            &self.model,
                            &tab,
                            area,
                            &self.ui,
                        )?,
                    },
                })
            }
            M::LayoutSetSplitRatio(p) => {
                let tab = p
                    .tab_id
                    .unwrap_or_else(|| self.model.active_tab().unwrap_or_default().into());
                self.model.tab_info(&tab)?;
                if let Some(address) = &p.pane_id {
                    let id = self.target(Some(address))?;
                    if self.model.context(id).is_none_or(|(_, t)| t.id != tab) {
                        return Err(invalid("split pane belongs to a different tab"));
                    }
                }
                if !p.ratio.is_finite() || !(0.05..=0.95).contains(&p.ratio) {
                    return Err(invalid("invalid Core split ratio"));
                }
                let anchor = self
                    .model
                    .layouts
                    .get(&tab)
                    .and_then(|l| split_members(l, &p.path))
                    .ok_or_else(|| invalid("Core split path not found"))?;
                self.change(|model| {
                    let layout = model
                        .layouts
                        .get_mut(&tab)
                        .ok_or_else(|| invalid("Core tab disappeared"))?;
                    if split_members(layout, &p.path).as_ref() != Some(&anchor) {
                        return Err(invalid("Core split boundary changed; retry the action"));
                    }
                    if !layout.set_ratio_at(&p.path, p.ratio) {
                        return Err(invalid("Core split path not found"));
                    }
                    Ok(())
                })?;
                Ok(R::LayoutSplitRatioSet {
                    layout: if self.model.active_tab() == Some(tab.as_str()) {
                        self.model.description()
                    } else {
                        self.model.description_for(&tab)
                    }
                    .ok_or_else(|| invalid("no Core layout"))?,
                })
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
                let layout_change = matches!(
                    &request.action,
                    actions::Action::PaneSplit(_)
                        | actions::Action::PaneClose(_)
                        | actions::Action::PaneSwap(_)
                        | actions::Action::PaneResize(_)
                        | actions::Action::LayoutSetSplitRatio(_)
                        | actions::Action::WorkspaceCreate(_)
                        | actions::Action::WorkspaceClose(_)
                        | actions::Action::TabCreate(_)
                        | actions::Action::TabClose(_)
                );
                let focus_change = layout_change
                    || matches!(
                        &request.action,
                        actions::Action::PaneFocus(_)
                            | actions::Action::PaneZoom(_)
                            | actions::Action::PaneFocusDirection(_)
                            | actions::Action::WorkspaceFocus(_)
                            | actions::Action::TabFocus(_)
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
                let can_be_noop = matches!(
                    &request.action,
                    actions::Action::PaneZoom(_) | actions::Action::PaneSwap(_)
                );
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
                        self.dirty |= !read_only
                            && !metadata_only
                            && (!can_be_noop || self.model.revision != before);
                        Ok(Box::new(result))
                    }
                    Err(error) => {
                        let swap_code = (name == "pane.swap")
                            .then(|| super::swap::error_code(&error))
                            .flatten();
                        let code = if let Some(code) = swap_code {
                            code
                        } else if error.to_string() == "stale_content" {
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
