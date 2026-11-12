use super::*;

/// Metadata for this client's sole Core connection. The projected snapshot is
/// kept separately from the cached metadata while a replacement surface arrives.
#[derive(Clone, Debug)]
pub(crate) struct ClientShellEndpoint {
    pub(crate) label: String,
    pub(crate) status: ClientEndpointStatus,
    pub(crate) snapshot: Option<Box<ClientShellSnapshot>>,
    pub(crate) snapshot_generation: Option<u64>,
    pub(crate) agent_recency: HashMap<String, u64>,
    pub(super) agent_presentation: super::endpoint_agent_state::EndpointAgentPresentation,
    pub(crate) methods: Option<HashSet<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClientEndpointFocusTarget {
    Workspace(String),
    Tab(String),
    Pane(String),
}

impl ClientShellState {
    pub(crate) fn set_endpoint_status(&mut self, status: ClientEndpointStatus) {
        if status != ClientEndpointStatus::Online {
            self.pending_workspace_highlight = None;
        }
        self.endpoint.status = status;
    }
    #[cfg(test)]
    pub(crate) fn mark_endpoint_disconnected(&mut self) {
        self.set_endpoint_status(ClientEndpointStatus::Reconnecting);
        let pending = self.pending_requests.keys().cloned().collect::<Vec<_>>();
        for request in pending {
            self.cancel_endpoint_request(&request);
        }
        self.pane_scroll_in_flight.clear();
        self.pane_scroll_queued.clear();
    }
    /// Adopt the advertised method set. Returns whether it actually changed.
    ///
    /// Open overlays keep the set they were built against, so a real change
    /// re-gates them here. An equal set leaves the user's highlight, settings
    /// section and theme preview untouched.
    pub(crate) fn set_endpoint_methods(&mut self, methods: Option<Vec<String>>) -> bool {
        let methods: Option<HashSet<String>> = methods.map(|methods| methods.into_iter().collect());
        if self.endpoint.methods == methods {
            return false;
        }
        self.endpoint.methods = methods;
        self.refresh_context_menu_methods();
        self.refresh_settings_sections();
        true
    }
    #[cfg(test)]
    pub(crate) fn activate_endpoint_projection(&mut self) -> bool {
        if self.endpoint.status != ClientEndpointStatus::Online {
            return false;
        }
        let Some(snapshot) = self.endpoint.snapshot.clone() else {
            return false;
        };
        self.apply_active_snapshot(snapshot, self.endpoint.snapshot_generation);
        true
    }
    pub(crate) fn endpoint_status(&self) -> ClientEndpointStatus {
        self.endpoint.status
    }

    pub(crate) fn endpoint_is_online(&self) -> bool {
        self.endpoint.status == ClientEndpointStatus::Online && self.endpoint.snapshot.is_some()
    }
    pub(crate) fn endpoint_label(&self) -> &str {
        &self.endpoint.label
    }
    pub(super) fn supports_endpoint_method(&self, method: &crate::actions::Action) -> bool {
        self.endpoint
            .methods
            .as_ref()
            .is_none_or(|m| m.contains(method.name()))
    }
    pub(super) fn focused_tab_count(&self) -> usize {
        self.snapshot.as_deref().map_or(0, |s| {
            s.tabs
                .iter()
                .filter(|t| Some(t.workspace_id.as_str()) == s.focused_workspace_id.as_deref())
                .count()
        })
    }
    #[cfg(test)]
    pub(crate) fn cache_endpoint_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        self.cache_endpoint_snapshot_with_surface(None, snapshot, true);
    }
    pub(crate) fn cache_endpoint_snapshot_for_generation(
        &mut self,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot_with_surface(Some(generation), snapshot, true);
    }
    #[cfg(test)]
    pub(crate) fn cache_endpoint_snapshot_inactive_for_generation(
        &mut self,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot_with_surface(Some(generation), snapshot, false);
    }
    /// `acknowledge` marks the projection as the live one: receiving it means
    /// this client's Core connection is serving, whatever it was doing before.
    fn cache_endpoint_snapshot_with_surface(
        &mut self,
        generation: Option<u64>,
        mut snapshot: Box<ClientShellSnapshot>,
        acknowledge: bool,
    ) {
        if acknowledge && self.endpoint.status != ClientEndpointStatus::Online {
            self.set_endpoint_status(ClientEndpointStatus::Online);
        }
        if self.endpoint.snapshot_generation == generation
            && self
                .endpoint
                .snapshot
                .as_deref()
                .is_some_and(|s| s.boot_id == snapshot.boot_id && s.revision > snapshot.revision)
        {
            return;
        }
        if self
            .endpoint
            .snapshot
            .as_deref()
            .is_some_and(|s| s.boot_id != snapshot.boot_id)
        {
            self.retire_endpoint_notifications();
        }
        self.endpoint
            .agent_presentation
            .project_snapshot(&mut snapshot);
        if let Some(surface) = self.pane_surface.as_ref().filter(|_| acknowledge) {
            self.endpoint.agent_presentation.acknowledge_surface(
                &mut snapshot,
                surface,
                self.outer_focused,
            );
        }
        let previous = self.endpoint.snapshot.as_deref();
        let mut recency = std::mem::take(&mut self.endpoint.agent_recency);
        let mut next = recency.values().copied().max().unwrap_or_default();
        let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
        agents.sort_by_key(|a| a.state_change_seq);
        for agent in agents {
            if previous
                .and_then(|s| s.agents.iter().find(|a| a.pane_id == agent.pane_id))
                .is_none_or(|a| a.state_change_seq != agent.state_change_seq)
            {
                next = next.saturating_add(1);
                recency.insert(agent.pane_id.clone(), next);
            }
        }
        recency.retain(|id, _| snapshot.agents.iter().any(|a| &a.pane_id == id));
        self.endpoint.agent_recency = recency;
        self.endpoint.snapshot_generation = generation;
        self.endpoint.snapshot = Some(snapshot);
    }
    pub(crate) fn acknowledge_active_surface_agents(&mut self, surface: &PaneSurfaceFrame) -> bool {
        let Some(snapshot) = self.endpoint.snapshot.as_deref_mut() else {
            return false;
        };
        let changed = self.endpoint.agent_presentation.acknowledge_surface(
            snapshot,
            surface,
            self.outer_focused,
        );
        if changed {
            self.snapshot = self.endpoint.snapshot.clone();
        }
        changed
    }
    #[cfg(test)]
    pub(crate) fn set_endpoint_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        self.cache_endpoint_snapshot(snapshot);
        self.apply_cached_snapshot();
    }
    /// Return whether the projection scope changed and needs a full host redraw.
    pub(crate) fn set_endpoint_snapshot_for_generation(
        &mut self,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) -> bool {
        let epoch_changed = self.active_snapshot_generation != Some(generation)
            || self
                .snapshot
                .as_ref()
                .is_none_or(|old| old.boot_id != snapshot.boot_id);
        self.cache_endpoint_snapshot_for_generation(generation, snapshot);
        self.apply_cached_snapshot();
        epoch_changed
    }
    #[cfg(test)]
    pub(crate) fn set_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        self.set_endpoint_snapshot(snapshot);
    }
    fn apply_cached_snapshot(&mut self) {
        if let Some(snapshot) = self.endpoint.snapshot.clone() {
            self.apply_active_snapshot(snapshot, self.endpoint.snapshot_generation);
        }
    }
}
pub(super) fn endpoint_status_presentation(
    status: ClientEndpointStatus,
    palette: &Palette,
) -> (&'static str, &'static str, ratatui::style::Color) {
    match status {
        ClientEndpointStatus::Connecting => ("◐", "connecting", palette.yellow),
        ClientEndpointStatus::Online => ("●", "online", palette.green),
        #[cfg(test)]
        ClientEndpointStatus::Reconnecting => ("◐", "reconnecting", palette.yellow),
    }
}
pub(super) fn local_endpoint() -> ClientShellEndpoint {
    ClientShellEndpoint {
        label: "Local".into(),
        status: ClientEndpointStatus::Connecting,
        snapshot: None,
        snapshot_generation: None,
        agent_recency: HashMap::new(),
        agent_presentation: Default::default(),
        methods: None,
    }
}
