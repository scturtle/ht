use super::*;

/// Metadata for this client's sole Core connection. The projected snapshot is
/// kept separately from the cached metadata while a replacement surface arrives.
#[derive(Clone, Debug)]
pub(crate) struct ClientShellEndpoint {
    pub(crate) endpoint_id: ClientEndpointId,
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
    pub(crate) fn set_endpoint_status(
        &mut self,
        _id: &ClientEndpointId,
        status: ClientEndpointStatus,
    ) {
        if status != ClientEndpointStatus::Online {
            self.pending_workspace_highlight = None;
        }
        self.endpoint.status = status;
    }
    #[cfg(test)]
    pub(crate) fn mark_endpoint_disconnected(&mut self, id: &ClientEndpointId) {
        self.set_endpoint_status(id, ClientEndpointStatus::Reconnecting);
        let pending = self.pending_requests.keys().cloned().collect::<Vec<_>>();
        for request in pending {
            self.cancel_endpoint_request(&request);
        }
        self.pending_integration_installs = 0;
        self.pane_scroll_in_flight.clear();
        self.pane_scroll_queued.clear();
    }
    pub(crate) fn set_endpoint_methods_for(
        &mut self,
        _id: &ClientEndpointId,
        methods: Option<Vec<String>>,
    ) {
        self.endpoint.methods = methods.map(|methods| methods.into_iter().collect());
    }
    #[cfg(test)]
    pub(crate) fn activate_endpoint_projection(&mut self, _id: &ClientEndpointId) -> bool {
        if self.endpoint.status != ClientEndpointStatus::Online {
            return false;
        }
        let Some(snapshot) = self.endpoint.snapshot.clone() else {
            return false;
        };
        self.apply_active_snapshot(snapshot, self.endpoint.snapshot_generation);
        true
    }
    pub(crate) fn endpoint_status(&self, _id: &ClientEndpointId) -> Option<ClientEndpointStatus> {
        Some(self.endpoint.status)
    }
    pub(crate) fn endpoint_is_online(&self, _id: &ClientEndpointId) -> bool {
        self.endpoint.status == ClientEndpointStatus::Online && self.endpoint.snapshot.is_some()
    }
    pub(crate) fn endpoint_label(&self, _id: &ClientEndpointId) -> &str {
        &self.endpoint.label
    }
    pub(crate) fn active_endpoint_label(&self) -> &str {
        &self.endpoint.label
    }
    pub(crate) fn endpoint_is_active(&self, _id: &ClientEndpointId) -> bool {
        true
    }
    #[cfg(test)]
    pub(crate) fn set_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        self.set_endpoint_snapshot(&ClientEndpointId::Local, snapshot);
    }
    #[cfg(test)]
    pub(crate) fn set_endpoint_methods(&mut self, methods: Option<Vec<String>>) {
        self.set_endpoint_methods_for(&ClientEndpointId::Local, methods);
    }
    pub(super) fn supports_endpoint_method(&self, method: &crate::api::schema::Method) -> bool {
        self.endpoint
            .methods
            .as_ref()
            .is_none_or(|m| m.contains(crate::api::api_method_name(method)))
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
    pub(crate) fn cache_endpoint_snapshot(
        &mut self,
        id: &ClientEndpointId,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot_with_surface(id, None, snapshot, true);
    }
    pub(crate) fn cache_endpoint_snapshot_for_generation(
        &mut self,
        id: &ClientEndpointId,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot_with_surface(id, Some(generation), snapshot, true);
    }
    #[cfg(test)]
    pub(crate) fn cache_endpoint_snapshot_inactive_for_generation(
        &mut self,
        id: &ClientEndpointId,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot_with_surface(id, Some(generation), snapshot, false);
    }
    fn cache_endpoint_snapshot_with_surface(
        &mut self,
        id: &ClientEndpointId,
        generation: Option<u64>,
        mut snapshot: Box<ClientShellSnapshot>,
        acknowledge: bool,
    ) {
        snapshot
            .commands
            .retain(|c| c.action != crate::protocol::ClientShellCommandAction::Unknown);
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
            self.retire_endpoint_notifications(id);
        }
        self.endpoint
            .agent_presentation
            .project_snapshot_for_generation(&mut snapshot, generation);
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
    pub(crate) fn set_endpoint_snapshot(
        &mut self,
        id: &ClientEndpointId,
        snapshot: Box<ClientShellSnapshot>,
    ) {
        self.cache_endpoint_snapshot(id, snapshot);
        self.apply_cached_snapshot();
    }
    /// Return whether the projection scope changed and needs a full host redraw.
    pub(crate) fn set_endpoint_snapshot_for_generation(
        &mut self,
        id: &ClientEndpointId,
        generation: u64,
        snapshot: Box<ClientShellSnapshot>,
    ) -> bool {
        let epoch_changed = self.active_snapshot_generation != Some(generation)
            || self
                .snapshot
                .as_ref()
                .is_none_or(|old| old.boot_id != snapshot.boot_id);
        self.cache_endpoint_snapshot_for_generation(id, generation, snapshot);
        self.apply_cached_snapshot();
        epoch_changed
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
        endpoint_id: ClientEndpointId::Local,
        label: "Local".into(),
        status: ClientEndpointStatus::Online,
        snapshot: None,
        snapshot_generation: None,
        agent_recency: HashMap::new(),
        agent_presentation: Default::default(),
        methods: None,
    }
}
