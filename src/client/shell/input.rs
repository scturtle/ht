use super::*;
use crate::protocol::ClientPaneInputEvent;
use crate::raw_input::RawInputEvent;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

const LOCAL_INPUT_SOURCE: u8 = 0;

/// True for Ctrl+[, the terminal-level equivalent of Esc.
///
/// A legacy terminal sends Ctrl+[ as 0x1b, the same byte as Esc, so it already
/// arrives as `KeyCode::Esc`. Under the kitty keyboard protocol the modified key
/// is reported on its own and reaches navigate mode as `Char('[')` with CONTROL.
/// Ctrl+Shift+[ stays distinct because it carries SHIFT.
///
/// Navigate mode checks this only after keybinding dispatch, so a configured
/// Ctrl+[ binding keeps working and this stays a fallback cancel.
fn is_ctrl_bracket_key(key: &crate::input::TerminalKey) -> bool {
    key.code == KeyCode::Char('[') && key.modifiers == KeyModifiers::CONTROL
}

fn host_theme_update(event: &RawInputEvent) -> Option<crate::protocol::ClientHostThemeUpdate> {
    use crate::protocol::{ClientHostDefaultColorKind, ClientHostThemeUpdate};

    match event {
        RawInputEvent::HostDefaultColor { kind, color } => {
            Some(ClientHostThemeUpdate::DefaultColor {
                kind: match kind {
                    crate::terminal::DefaultColorKind::Foreground => {
                        ClientHostDefaultColorKind::Foreground
                    }
                    crate::terminal::DefaultColorKind::Background => {
                        ClientHostDefaultColorKind::Background
                    }
                },
                color: (*color).into(),
            })
        }
        RawInputEvent::HostPaletteColors { colors } => Some(ClientHostThemeUpdate::PaletteColors(
            colors
                .iter()
                .map(|(index, color)| (*index, (*color).into()))
                .collect(),
        )),
        _ => None,
    }
}

fn push_host_theme_update(
    requests: &mut Vec<ClientMessage>,
    update: crate::protocol::ClientHostThemeUpdate,
) {
    if let crate::protocol::ClientHostThemeUpdate::PaletteColors(colors) = &update {
        if let Some(ClientMessage::ClientShellHostTheme {
            update: crate::protocol::ClientHostThemeUpdate::PaletteColors(pending),
        }) = requests.last_mut()
        {
            if pending.len() + colors.len() <= 256 {
                pending.extend_from_slice(colors);
                return;
            }
        }
    }
    requests.push(ClientMessage::ClientShellHostTheme { update });
}

impl ClientShellState {
    pub(crate) fn host_keyboard_report_all_requested(&self) -> bool {
        matches!(
            self.mode,
            ClientShellMode::Prefix | ClientShellMode::Navigate
        )
    }

    pub(crate) fn set_host_reports_key_releases(&mut self, reports: bool) {
        self.host_reports_key_releases = reports;
    }

    pub(crate) fn set_host_reports_all_keys(&mut self, reports_all: bool) {
        self.host_reports_all_keys = reports_all;
    }

    /// While the pane asks for every key as an escape code, committed text
    /// (IME, compose) must reach it as text, not as invented key reports.
    /// On a Kitty host switched to report-all, typed keys arrive as reports,
    /// so any plain text is a commit (kitty sends those as text too). A legacy
    /// host sends both as plain text; non-ASCII text has no key in the
    /// keyboard model, so only that is treated as a commit there.
    /// Only text bound for the focused pane is a commit. In prefix, navigate
    /// and other Herdr-owned input, plain text is still a key that may run a
    /// binding (#5255).
    fn host_text_commit(&self, key: &crate::input::TerminalKey) -> Option<String> {
        if !self.host_reports_all_keys
            || key.kind != KeyEventKind::Press
            || self.mode != ClientShellMode::Terminal
            || self.overlay.is_some()
        {
            return None;
        }
        let plain_text = key
            .vt_bytes()
            .is_some_and(|bytes| bytes.first().is_some_and(|byte| *byte != 0x1b));
        let text = key.generated_text.as_ref().filter(|_| plain_text)?;
        (self.host_reports_key_releases || !text.is_ascii()).then(|| text.clone())
    }

    pub(crate) fn set_host_erase_byte(&mut self, erase: Option<u8>) {
        self.host_erase_is_ctrl_h = erase == Some(0x08);
    }

    /// A raw 0x08 (or ESC 0x08) from a host whose erase character is `^H` is
    /// its Backspace key (#3244).
    fn host_erase_key(&self, key: crate::input::TerminalKey) -> crate::input::TerminalKey {
        if !self.host_erase_is_ctrl_h {
            return key;
        }
        let alt = match key.vt_bytes() {
            Some([0x08]) => KeyModifiers::empty(),
            Some([0x1b, 0x08]) => KeyModifiers::ALT,
            _ => return key,
        };
        let bytes = key.vt_bytes().map(<[u8]>::to_vec).unwrap_or_default();
        crate::input::TerminalKey::new(KeyCode::Backspace, alt)
            .with_kind(key.kind)
            .with_vt_bytes(bytes)
    }

    #[cfg(test)]
    pub(crate) fn handle_input_bytes(&mut self, data: &[u8]) -> ClientShellInput {
        self.handle_raw_events(crate::raw_input::parse_framed_input(data))
    }

    pub(crate) fn handle_pixel_mouse(
        &mut self,
        data: &[u8],
        geometry: crate::client::host_terminal::HostGeometry,
    ) -> ClientShellInput {
        let Some(report) = crate::raw_input::parse_sgr_mouse_report(data) else {
            return ClientShellInput::default();
        };
        let Some((column, row)) = geometry.cell(report.x, report.y) else {
            return ClientShellInput::default();
        };
        self.host_mouse_pixels = Some(crate::client::host_terminal::HostPixels {
            x: report.x,
            y: report.y,
            geometry,
        });
        let outcome =
            self.handle_raw_events(vec![RawInputEvent::Mouse(report.at_cell(column, row))]);
        self.host_mouse_pixels = None;
        outcome
    }

    fn prepare_committed_text(&mut self, text: &str, outcome: &mut ClientShellInput) -> bool {
        if !(self.mode == ClientShellMode::Navigate && self.workspace_preview_action_blocked())
            && self.insert_copy_search_text(text)
        {
            outcome.repaint = true;
            return true;
        }
        self.word_selection_gesture = None;
        if self.copy_or_terminal_mode() != ClientShellMode::Copy && self.selection.take().is_some()
        {
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
            outcome.repaint = true;
        }
        false
    }

    pub(crate) fn replay_mouse_events(
        &mut self,
        events: Vec<crossterm::event::MouseEvent>,
    ) -> ClientShellInput {
        self.replaying_url_click = true;
        let outcome =
            self.handle_raw_events(events.into_iter().map(RawInputEvent::Mouse).collect());
        self.replaying_url_click = false;
        outcome
    }

    pub(crate) fn handle_raw_events(&mut self, events: Vec<RawInputEvent>) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        if !events.is_empty() && self.endpoint_error.take().is_some() {
            self.endpoint_error_deadline = None;
            outcome.repaint = true;
        }
        for event in events {
            let event = match event {
                RawInputEvent::Key(key) => match self.host_text_commit(&key) {
                    Some(text) => RawInputEvent::Text(crate::input::TextCommit::new(text)),
                    None => RawInputEvent::Key(key),
                },
                other => other,
            };
            if let Some(update) = host_theme_update(&event) {
                push_host_theme_update(&mut outcome.requests, update);
            }
            match event {
                RawInputEvent::Key(key) => {
                    let key = self.host_erase_key(key);
                    self.handle_key(key, &mut outcome)
                }
                RawInputEvent::Text(text) => {
                    let text = text.into_string();
                    if self.prepare_committed_text(&text, &mut outcome) {
                        continue;
                    }
                    if self.insert_overlay_text(&text) {
                        outcome.repaint = true;
                    } else if self.overlay.is_none() && self.mode == ClientShellMode::Terminal {
                        self.push_focused_pane_event(
                            ClientPaneInputEvent::TextCommit(text),
                            &mut outcome,
                        );
                    }
                }
                RawInputEvent::Paste(text) => {
                    if self.prepare_committed_text(&text, &mut outcome) {
                        continue;
                    }
                    if self.insert_overlay_text(&text) {
                        outcome.repaint = true;
                    } else if self.overlay.is_none() && self.mode == ClientShellMode::Terminal {
                        self.push_focused_pane_event(
                            ClientPaneInputEvent::Paste(text),
                            &mut outcome,
                        );
                    }
                }
                RawInputEvent::Mouse(mouse) => self.handle_mouse(mouse, &mut outcome),
                RawInputEvent::OuterFocusGained => {
                    self.outer_focused = Some(true);
                    outcome.repaint |= self.config.redraw_on_focus_gained;
                    if let Some(surface) = self.pane_surface.clone() {
                        outcome.repaint |= self.acknowledge_active_surface_agents(
                            &surface,
                            self.pane_surface_generation,
                        );
                    }
                    outcome
                        .requests
                        .push(ClientMessage::ClientShellFocus { focused: true });
                }
                RawInputEvent::OuterFocusLost => {
                    outcome.repaint |= self.clear_link_hover();
                    self.outer_focused = Some(false);
                    self.release_input_leases(&mut outcome);
                    outcome
                        .requests
                        .push(ClientMessage::ClientShellFocus { focused: false });
                }
                RawInputEvent::HostDefaultColor {
                    kind: crate::terminal::DefaultColorKind::Background,
                    color,
                } => {
                    if self.host_background != Some(color) {
                        self.host_background = Some(color);
                        outcome.repaint = true;
                    }
                }
                RawInputEvent::HostDefaultColor { .. }
                | RawInputEvent::HostPaletteColors { .. }
                | RawInputEvent::HostCellSizeReport { .. }
                | RawInputEvent::Unsupported => {}
            }
        }
        outcome.repaint |= self.resume_mobile_switcher_if_ready();
        outcome
    }

    pub(super) fn handle_key(
        &mut self,
        key: crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint |= self.clear_link_hover();
        if self.copy_operation_in_flight {
            self.copy_input_queue.push_back(key);
            return;
        }
        let key = if self.host_reports_key_releases && key.generated_text.is_some() {
            key.with_physical_identity_hint(true)
        } else {
            key
        };
        let lease_key = crate::input::InputLeaseKey::new(LOCAL_INPUT_SOURCE, &key);
        let key = self.input_leases.normalize_press(&lease_key, key);
        match key.kind {
            KeyEventKind::Press => {
                let initial_context = self.input_context();
                let target = self.route_key_press(&key, outcome);
                if let Some(target) = target.as_ref() {
                    self.push_pane_key(target.clone(), key.clone(), outcome);
                }
                let resulting_context = self.input_context();
                let plan = self.input_leases.complete_press(
                    lease_key,
                    &key,
                    Some(&initial_context),
                    Some(&resulting_context),
                    target,
                );
                self.execute_repeat_plan(lease_key, key, plan, outcome);
            }
            KeyEventKind::Repeat => {
                let context = self.input_context();
                let plan = self
                    .input_leases
                    .plan_repeat(lease_key, &key, Some(&context));
                self.execute_repeat_plan(lease_key, key, plan, outcome);
            }
            KeyEventKind::Release => {
                let Some(lease) = self.take_release_lease(&lease_key, &key) else {
                    return;
                };
                let crate::input::InputLease::Forwarded(lease) = lease else {
                    // Herdr consumed the press; its release stays with Herdr.
                    return;
                };
                // A native record is released as the recorded key. A VT release
                // report already names its key and shifted character exactly,
                // except around a non-Latin Ctrl chord: Ctrl alone resolves to
                // the physical key, so a modifier let go before the key flips
                // between the layout character and that key. It is released as
                // the key the pane saw pressed.
                let layout_chord =
                    key.code != lease.key.code && key.identity() == lease.key.identity();
                let release = if key.physical_key_id().is_some() || layout_chord {
                    lease
                        .key
                        .with_modifiers(key.modifiers)
                        .with_kind(KeyEventKind::Release)
                } else {
                    key
                };
                self.push_pane_key(lease.target, release, outcome);
            }
        }
    }

    fn release_input_leases(&mut self, outcome: &mut ClientShellInput) {
        for lease in self.input_leases.remove_source(LOCAL_INPUT_SOURCE) {
            // A press that arrived as plain text (IME commits, and printable keys
            // the host sends as text) reached the pane as text, not as a key
            // event. IME commits never get a host release, so inventing one per
            // character is noise. Key reports, with or without text, still get
            // their release.
            let plain_text_press = lease.key.generated_text.is_some()
                && lease
                    .key
                    .vt_bytes()
                    .is_some_and(|bytes| bytes.first().is_some_and(|byte| *byte != 0x1b));
            if plain_text_press {
                continue;
            }
            self.push_pane_key(
                lease.target,
                lease.key.with_kind(KeyEventKind::Release),
                outcome,
            );
        }
        if let Some(gesture) = self.pane_mouse_gesture.take() {
            let modifiers = gesture
                .last_event
                .modifiers
                .difference(gesture.stripped_modifiers);
            let geometry = matches!(
                gesture.last_position,
                crate::protocol::ClientMousePosition::Pixels { .. }
            )
            .then_some(crate::protocol::ClientMouseGeometry {
                cols: gesture.hit.inner_rect.width,
                rows: gesture.hit.inner_rect.height,
                width_px: gesture.hit.pixel_width,
                height_px: gesture.hit.pixel_height,
            });
            super::push_pane_event(
                gesture.hit.pane_id,
                ClientPaneInputEvent::Mouse {
                    kind: crate::protocol::ClientMouseKind::Up(
                        crate::protocol::ClientMouseButton::from_crossterm(gesture.button),
                    ),
                    position: gesture.last_position,
                    geometry,
                    modifiers: modifiers.bits(),
                    lines: self.config.mouse_scroll_lines.min(u16::MAX as usize) as u16,
                },
                outcome,
            );
        }
        self.copy_input_queue.clear();
    }

    fn execute_repeat_plan(
        &mut self,
        lease_key: crate::input::InputLeaseKey<u8>,
        key: crate::input::TerminalKey,
        plan: crate::input::RepeatPlan<ClientInputContext, String>,
        outcome: &mut ClientShellInput,
    ) {
        match plan {
            crate::input::RepeatPlan::Forwarded(target) => {
                self.push_pane_key(target, key, outcome);
            }
            crate::input::RepeatPlan::Reprocess {
                context,
                repetitions,
                tracked,
            } => {
                for _ in 0..repetitions {
                    let current = self.input_context();
                    if !self.input_leases.reprocess_allowed(
                        lease_key,
                        &context,
                        Some(&current),
                        tracked,
                    ) {
                        break;
                    }
                    let repeated = key
                        .clone()
                        .with_repeat_count(1)
                        .with_kind(KeyEventKind::Repeat);
                    if let Some(target) = self.route_key_press(&repeated, outcome) {
                        self.push_pane_key(target, repeated, outcome);
                    }
                }
            }
            crate::input::RepeatPlan::Ignore => {}
        }
    }

    fn route_key_press(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> Option<String> {
        if self.overlay.is_some() {
            self.route_overlay_key(key, outcome);
            return None;
        }
        if matches!(key.code, KeyCode::Modifier(_)) {
            return None;
        }
        self.word_selection_gesture = None;
        if self.mode != ClientShellMode::Copy
            && self.copy_or_terminal_mode() != ClientShellMode::Copy
            && self.selection.take().is_some()
        {
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
            outcome.repaint = true;
        }

        match self.mode {
            ClientShellMode::Terminal => {
                if let Some(binding) =
                    keybindings::resolve_direct_binding(&self.config.keybinds.keybinds, key)
                {
                    self.record_binding(binding, outcome);
                    return None;
                }
                if self.config.keybinds.matches_prefix(key) {
                    self.mode = ClientShellMode::Prefix;
                    outcome.repaint = true;
                    return None;
                }
                self.focused_pane_id()
            }
            ClientShellMode::Prefix => {
                let return_mode = if self.copy_mode.as_ref().is_some_and(|copy_mode| {
                    self.focused_pane_id().as_deref() == Some(copy_mode.pane_id.as_str())
                }) {
                    ClientShellMode::Copy
                } else {
                    ClientShellMode::Terminal
                };
                if self.config.keybinds.matches_prefix(key) {
                    self.mode = return_mode;
                    outcome.repaint = true;
                    return self.focused_pane_id();
                }
                if key.code == KeyCode::Esc {
                    self.mode = return_mode;
                    outcome.repaint = true;
                    return None;
                }
                if let Some(binding) =
                    keybindings::resolve_prefix_binding(&self.config.keybinds.keybinds, key)
                {
                    self.mode = return_mode;
                    outcome.repaint = true;
                    self.record_binding(binding, outcome);
                    return None;
                }
                self.mode = return_mode;
                outcome.repaint = true;
                None
            }
            ClientShellMode::Navigate => {
                self.route_navigate_key(key, outcome);
                None
            }
            ClientShellMode::Copy => {
                if self
                    .copy_mode
                    .as_ref()
                    .is_none_or(|copy_mode| copy_mode.search_prompt.is_none())
                    && self.config.keybinds.matches_prefix(key)
                {
                    self.mode = ClientShellMode::Prefix;
                    outcome.repaint = true;
                } else {
                    self.route_copy_mode_key(key, outcome);
                }
                None
            }
        }
    }

    pub(super) fn copy_or_terminal_mode(&self) -> ClientShellMode {
        if self.copy_mode.as_ref().is_some_and(|copy_mode| {
            self.focused_pane_id().as_deref() == Some(copy_mode.pane_id.as_str())
        }) {
            ClientShellMode::Copy
        } else {
            ClientShellMode::Terminal
        }
    }

    fn route_navigate_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        use keybindings::{KeybindDispatch, KeybindMatch};

        self.pending_workspace_highlight = None;
        if key.code == KeyCode::Esc || self.config.keybinds.matches_prefix(key) {
            self.cancel_navigate(outcome);
            return;
        }

        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if modifiers.is_empty() {
            match code {
                KeyCode::Up | KeyCode::Down => {
                    self.move_navigate_workspace(if code == KeyCode::Up { -1 } else { 1 });
                    outcome.repaint = true;
                    return;
                }
                KeyCode::Enter => {
                    self.accept_navigate_workspace(outcome);
                    return;
                }
                _ => {}
            }
        }
        if self.workspace_preview_action_blocked() {
            // Bindings cannot run against a foreign preview, so Ctrl+[ has
            // nothing to lose to and cancels here the same way Esc does.
            if is_ctrl_bracket_key(key) {
                self.cancel_navigate(outcome);
                return;
            }
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Rejected,
                "navigate_endpoint_inactive",
                "Confirm workspace first",
                "Select an available workspace and press Enter before using workspace or pane actions",
            );
            outcome.repaint = true;
            return;
        }

        if let Some(index) = ('1'..='9').position(|digit| {
            crate::config::terminal_key_matches_combo(
                key,
                (KeyCode::Char(digit), KeyModifiers::empty()),
            )
        }) {
            if let Some(workspace_id) = self.endpoint.snapshot.as_deref().and_then(|snapshot| {
                let entries = navigation_rows::workspace_entries(snapshot);
                let entry = entries.get(index)?;
                Some(snapshot.workspaces[entry.index].workspace_id.clone())
            }) {
                self.mode = ClientShellMode::Terminal;
                self.navigate_workspace_id = None;
                self.push_endpoint_method(
                    crate::actions::Action::WorkspaceFocus(crate::actions::WorkspaceTarget {
                        workspace_id,
                    }),
                    outcome,
                );
                outcome.repaint = true;
            }
            return;
        }

        if modifiers.is_empty() && matches!(code, KeyCode::Tab | KeyCode::BackTab) {
            self.cycle_pane(code == KeyCode::BackTab, outcome);
            self.cancel_navigate(outcome);
            return;
        }

        let binding = keybindings::resolve_non_indexed_action(
            &self.config.keybinds.keybinds,
            key,
            KeybindDispatch::Prefix,
        )
        .map(KeybindMatch::Action)
        .or_else(|| {
            keybindings::resolve_indexed_action(
                &self.config.keybinds.keybinds,
                key,
                KeybindDispatch::Prefix,
            )
            .map(KeybindMatch::Action)
        });
        if let Some(binding) = binding {
            self.record_navigate_binding(binding, outcome);
        } else if is_ctrl_bracket_key(key) {
            self.cancel_navigate(outcome);
        }
    }

    fn cancel_navigate(&mut self, outcome: &mut ClientShellInput) {
        self.mode = self.copy_or_terminal_mode();
        self.navigate_workspace_id = None;
        outcome.repaint = true;
    }

    fn record_navigate_binding(
        &mut self,
        binding: keybindings::KeybindMatch,
        outcome: &mut ClientShellInput,
    ) {
        if !self.indexed_navigation_target_exists(&binding) {
            return;
        }
        self.mode = ClientShellMode::Terminal;
        self.record_binding(binding, outcome);
        if self.mode == ClientShellMode::Navigate {
            self.mode = self.copy_or_terminal_mode();
        }
        self.navigate_workspace_id = None;
        outcome.repaint = true;
    }

    fn indexed_navigation_target_exists(&self, binding: &keybindings::KeybindMatch) -> bool {
        use keybindings::{KeybindAction, KeybindMatch};

        match binding {
            KeybindMatch::Action(KeybindAction::SwitchTab(index)) => self
                .endpoint
                .snapshot
                .as_deref()
                .and_then(|snapshot| {
                    let workspace_id = snapshot.focused_workspace_id.as_deref()?;
                    snapshot
                        .tabs
                        .iter()
                        .filter(|tab| tab.workspace_id == workspace_id)
                        .nth(*index)
                })
                .is_some(),
            _ => true,
        }
    }

    fn cycle_pane(&mut self, reverse: bool, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.endpoint.snapshot.as_deref() else {
            return;
        };
        let Some(tab_id) = snapshot.focused_tab_id.as_deref() else {
            return;
        };
        // The retained surface contains only the revealed pane; the snapshot
        // lists all panes in the current tab, including hidden siblings.
        let panes: Vec<_> = snapshot
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab_id)
            .collect();
        if panes.is_empty() {
            return;
        }
        let current = panes
            .iter()
            .position(|pane| Some(pane.pane_id.as_str()) == snapshot.focused_pane_id.as_deref())
            .unwrap_or(0);
        let next = if reverse {
            (current + panes.len() - 1) % panes.len()
        } else {
            (current + 1) % panes.len()
        };
        let pane_id = panes[next].pane_id.clone();
        self.push_endpoint_method(
            crate::actions::Action::PaneFocus(crate::actions::PaneTarget { pane_id }),
            outcome,
        );
    }

    fn input_context(&self) -> ClientInputContext {
        ClientInputContext {
            mode: self.mode,
            overlay: self.overlay.as_ref().map(ClientShellOverlay::kind),
            retained_selection: self
                .selection
                .as_ref()
                .is_some_and(crate::selection::Selection::is_visible),
        }
    }

    pub(super) fn focused_pane_id(&self) -> Option<String> {
        self.endpoint
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_pane_id.clone())
    }

    /// The tracked press for a release. A press that arrived as text ("A") is
    /// reported released as its key ("a", shifted alternate "A"), possibly after
    /// Shift was let go, so the shifted alternate and the letter's other case
    /// also match.
    fn take_release_lease(
        &mut self,
        lease_key: &crate::input::InputLeaseKey<u8>,
        key: &crate::input::TerminalKey,
    ) -> Option<crate::input::InputLease<ClientInputContext, String>> {
        if let Some(lease) = self.input_leases.remove(lease_key) {
            return Some(lease);
        }
        let KeyCode::Char(c) = key.layout_key().map_or(key.code, KeyCode::Char) else {
            return None;
        };
        fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
            match (chars.next(), chars.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
        let other_case = if c.is_lowercase() {
            single(c.to_uppercase())
        } else {
            single(c.to_lowercase())
        }
        .filter(|other| *other != c);
        key.shifted_codepoint
            .and_then(char::from_u32)
            .into_iter()
            .chain(other_case)
            .find_map(|candidate| {
                self.input_leases.remove(&crate::input::InputLeaseKey::new(
                    LOCAL_INPUT_SOURCE,
                    &crate::input::TerminalKey::new(KeyCode::Char(candidate), key.modifiers),
                ))
            })
            // Shifted punctuation ("?") released after Shift: the report names
            // only the unshifted key ("/"), which depends on the layout.
            .or_else(|| self.input_leases.remove_sole_text_press(LOCAL_INPUT_SOURCE))
    }

    fn push_pane_key(
        &self,
        pane_id: String,
        key: crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        if let Some(event) = ClientPaneInputEvent::from_terminal_key(key) {
            super::push_pane_event(pane_id, event, outcome);
        }
    }

    fn push_focused_pane_event(&self, event: ClientPaneInputEvent, outcome: &mut ClientShellInput) {
        if let Some(pane_id) = self.focused_pane_id() {
            super::push_pane_event(pane_id, event, outcome);
        }
    }
}
