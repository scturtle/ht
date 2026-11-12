use super::*;
use serde_json::json;

impl ClientShellState {
    pub(super) fn dump_pane_diagnostics(&self, pane_id: String, outcome: &mut ClientShellInput) {
        let snapshot = self.snapshot.as_deref();
        let surface = self.pane_surface.as_ref();
        let pane = surface.and_then(|s| s.panes.iter().find(|p| p.pane_id == pane_id));
        let cells = pane.zip(surface).map(|(pane, surface)| {
            let frame = &surface.frame;
            let rect = pane.inner_rect;
            (rect.y..rect.y.saturating_add(rect.height).min(frame.height))
                .map(|y| {
                    let start = usize::from(y) * usize::from(frame.width)
                        + usize::from(rect.x.min(frame.width));
                    let end = usize::from(y) * usize::from(frame.width)
                        + usize::from(rect.x.saturating_add(rect.width).min(frame.width));
                    frame.cells.get(start..end).unwrap_or_default().to_vec()
                })
                .collect::<Vec<_>>()
        });
        let client = json!({
            "trigger":"menu", "pane_id":pane_id, "mode":format!("{:?}",self.mode),
            "outer_focused":self.outer_focused, "endpoint":"local",
            "workspace_preview_blocked":self.workspace_preview_action_blocked(),
            "navigate_workspace":self.navigate_workspace_id.as_ref().map(|t| json!({"endpoint":"local","workspace":t.workspace_id,"scope_debug":format!("{t:?}")})),
            "snapshot_generation":self.active_snapshot_generation,
            "surface_generation":self.pane_surface_generation,
            "snapshot_revision":snapshot.map(|s| s.revision),
            "boot_id":snapshot.map(|s| &s.boot_id),
            "focused_pane":snapshot.and_then(|s| s.focused_pane_id.as_ref()),
            "pane":snapshot.and_then(|s| s.panes.iter().find(|p| p.pane_id==pane_id)),
            "surface":surface.map(|s| json!({"projection_revision":s.projection_revision,"surface_revision":s.surface_revision,"size":[s.frame.width,s.frame.height],"cursor":s.frame.cursor})),
            "surface_pane":pane, "pane_cells":cells,
            "pending_surface":self.pending_pane_surface.as_ref().map(|s| json!({"projection_revision":s.projection_revision,"surface_revision":s.surface_revision})),
            "pending_requests":self.pending_requests.iter().map(|(id,r)| json!({"id":id,"boot_id":r.boot_id,"method":r.method_name})).collect::<Vec<_>>(),
            "copy_mode":self.copy_mode.is_some(), "copy_operation_in_flight":self.copy_operation_in_flight,
            "copy_operation_queue":self.copy_operation_queue.len(), "copy_input_queue":self.copy_input_queue.len(),
            "scroll_in_flight":self.pane_scroll_in_flight.get(&pane_id),
            "scroll_queued":self.pane_scroll_queued.get(&pane_id), "scroll_target":self.pane_scroll_targets.get(&pane_id),
            "input_leases":self.input_leases.len(), "pane_mouse_gesture":self.pane_mouse_gesture.is_some(),
            "word_selection_gesture":self.word_selection_gesture.is_some(), "chrome_drag":self.chrome_drag.is_some(),
            "last_composed_size":self.last_composed_size,
            "last_composed_age_ms":self.last_composed_at.map(|t| t.elapsed().as_millis()),
            "host_reports_all_keys":self.host_reports_all_keys,"host_reports_releases":self.host_reports_key_releases,
        });
        outcome
            .actions
            .push(ClientShellAction::DumpPaneDiagnostics {
                pane_id,
                client: Box::new(client),
            });
    }

    pub(crate) fn diagnostic_error(&mut self, message: String) -> bool {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Diagnostic,
            "dump-error",
            "Cannot save pane diagnostics",
            message,
        )
    }

    pub(crate) fn diagnostic_notice(
        &mut self,
        path: &std::path::Path,
        warning: Option<&str>,
    ) -> bool {
        self.push_endpoint_notice(
            ClientEndpointNoticeKind::Diagnostic,
            format!("dump:{}", path.display()),
            if warning.is_some() {
                "Pane diagnostics"
            } else {
                "Pane diagnostics saved"
            },
            format!(
                "{}\n{}",
                path.display(),
                warning.unwrap_or("Contains pane screen text; keep private.")
            ),
        )
    }
}
