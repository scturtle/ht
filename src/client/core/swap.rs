//! The original pane-swap API policy over the retained pure TileLayout.
//! Recomputed after each CAS rebase; never spawns, kills or replays input.
use super::model::{invalid, Model};
use crate::{api::schema as api, layout::PaneId};
use ratatui::layout::Rect;
use std::io;

const INVALID_SHAPE: &str =
    "provide either direction with optional pane_id, or source_pane_id and target_pane_id";
const NO_LAYOUT: &str = "pane layout unavailable";
const NO_SOURCE: &str = "source pane not found";

pub(super) fn error_code(error: &io::Error) -> Option<&'static str> {
    match error.to_string().as_str() {
        INVALID_SHAPE | "missing source_pane_id" | "missing target_pane_id" => {
            Some("invalid_pane_swap")
        }
        NO_SOURCE => Some("pane_not_found"),
        NO_LAYOUT => Some("pane_layout_unavailable"),
        _ => None,
    }
}

/// Pin the omitted source once, before CAS retries. A retired source must fail,
/// never silently redirect the gesture to a newly focused surviving program.
pub(super) fn pin_source(
    model: &Model,
    mut params: api::PaneSwapParams,
) -> io::Result<api::PaneSwapParams> {
    let explicit = params.source_pane_id.is_some() || params.target_pane_id.is_some();
    if params.direction.is_some() == explicit {
        return Err(invalid(INVALID_SHAPE));
    }
    if params.direction.is_some() && params.pane_id.is_none() {
        params.pane_id = Some(
            model
                .focused()
                .and_then(|id| model.records.get(&id))
                .ok_or_else(|| invalid(NO_SOURCE))?
                .address
                .clone(),
        );
    }
    Ok(params)
}

impl Model {
    pub(super) fn swap_panes(
        &mut self,
        params: &api::PaneSwapParams,
        area: Rect,
    ) -> io::Result<api::PaneSwapResult> {
        use api::PaneSwapReason as Reason;
        let directional = params.direction.is_some();
        let explicit = params.source_pane_id.is_some() || params.target_pane_id.is_some();
        if directional == explicit {
            return Err(invalid(INVALID_SHAPE));
        }
        let context_tab = |model: &Model, id: PaneId| model.context(id).map(|(_, t)| t.id.clone());
        let (tab, source, target, reason) = if let Some(direction) = params.direction {
            let source = match params.pane_id.as_deref() {
                Some(address) => self.id(address),
                None => self.focused(),
            }
            .ok_or_else(|| invalid(NO_SOURCE))?;
            let tab = context_tab(self, source).ok_or_else(|| invalid(NO_SOURCE))?;
            let layout = self.layouts.get(&tab).ok_or_else(|| invalid(NO_LAYOUT))?;
            let panes = layout.panes(area); // Original directional lookup uses the hidden BSP when zoomed.
            let source_info = panes
                .iter()
                .find(|p| p.id == source)
                .ok_or_else(|| invalid(NO_SOURCE))?;
            let target = crate::layout::find_in_direction(
                source_info,
                super::methods::nav(direction),
                &panes,
            );
            let reason = target.is_none().then_some(Reason::NoNeighbor);
            (tab, source, target, reason)
        } else {
            let source_raw = params
                .source_pane_id
                .as_deref()
                .ok_or_else(|| invalid("missing source_pane_id"))?;
            let target_raw = params
                .target_pane_id
                .as_deref()
                .ok_or_else(|| invalid("missing target_pane_id"))?;
            let source = self.id(source_raw);
            let target = self.id(target_raw);
            // Preserve original response context and raw invalid IDs in no-op replies.
            let tab = source
                .and_then(|id| context_tab(self, id))
                .or_else(|| target.and_then(|id| context_tab(self, id)))
                .or_else(|| self.active_tab().map(str::to_owned))
                .ok_or_else(|| invalid(NO_LAYOUT))?;
            let reason = match (source, target) {
                (None, _) | (_, None) => Some(Reason::NotFound),
                (Some(source), Some(target)) if source == target => Some(Reason::SamePane),
                (Some(source), Some(target))
                    if context_tab(self, source) != context_tab(self, target) =>
                {
                    Some(Reason::CrossTab)
                }
                _ => None,
            };
            let source = source
                .or_else(|| self.layouts.get(&tab).map(|l| l.focused()))
                .ok_or_else(|| invalid(NO_LAYOUT))?;
            (tab, source, target, reason)
        };
        let mut changed = false;
        if reason.is_none() {
            if let Some(target) = target {
                changed = self
                    .layouts
                    .get_mut(&tab)
                    .ok_or_else(|| invalid(NO_LAYOUT))?
                    .swap_panes(source, target);
                if changed {
                    self.focus(source)?;
                }
            }
        }
        let layout = self.layout_snapshot_for(&tab, area)?;
        Ok(api::PaneSwapResult {
            changed,
            reason,
            source_pane_id: params
                .source_pane_id
                .clone()
                .unwrap_or_else(|| self.records[&source].address.clone()),
            target_pane_id: params
                .target_pane_id
                .clone()
                .or_else(|| target.map(|id| self.records[&id].address.clone())),
            focused_pane_id: layout.focused_pane_id.clone(),
            layout,
        })
    }
}

#[cfg(test)]
#[path = "swap_tests.rs"]
mod tests;
