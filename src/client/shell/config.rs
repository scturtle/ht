use super::*;

pub(super) fn merged_config_diagnostic(
    local: Option<&str>,
    endpoint: Option<&str>,
) -> Option<String> {
    match (local, endpoint) {
        (Some(local), Some(endpoint)) if local == endpoint => {
            Some(format!("client + endpoint: {local}"))
        }
        (Some(local), Some(endpoint)) => Some(format!("client: {local}\nendpoint: {endpoint}")),
        (Some(local), None) => Some(local.to_owned()),
        (None, Some(endpoint)) => Some(endpoint.to_owned()),
        (None, None) => None,
    }
}

impl ClientShellState {
    pub(super) fn set_local_config_diagnostic(&mut self, diagnostic: Option<String>) {
        self.local_config_diagnostic = diagnostic;
        self.config_diagnostic = merged_config_diagnostic(
            self.local_config_diagnostic.as_deref(),
            self.endpoint
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.config_diagnostic.as_deref()),
        );
    }

    pub(crate) fn reload_client_config(&mut self) {
        match crate::config::load_live_config() {
            Ok(loaded) => {
                let diagnostics = self.config.apply_live_config(
                    &loaded.config,
                    &loaded.diagnostics,
                    &loaded.invalid_sections,
                );
                self.set_local_config_diagnostic(crate::config::config_diagnostic_summary(
                    &diagnostics,
                ));
            }
            Err(diagnostics) => {
                self.set_local_config_diagnostic(crate::config::config_diagnostic_summary(
                    &diagnostics,
                ));
            }
        }
    }
}

impl ClientShellConfig {
    pub(crate) fn from_config(config: &Config) -> Self {
        let keybinds = config
            .live_keybinds_with_diagnostics()
            .map(|(keybinds, _diagnostics)| keybinds)
            .unwrap_or_else(|_diagnostics| LiveKeybindConfig {
                prefix: config.prefix_keys(),
                keybinds: config.keybinds(),
            });
        Self {
            mobile_width_threshold: config.ui.mobile_width_threshold,
            tab_bar_position: config.ui.tab_bar_position,
            agent_panel_sort: config.ui.agent_panel_sort,
            status_indicators: config.ui.status_indicators,
            toast_delivery: config.ui.toast.delivery,
            toast_delay_seconds: config.ui.toast.delay_seconds,
            toast_position: config.ui.toast.herdr.position,
            clipboard_toast_enabled: config.ui.toast.clipboard.enabled,
            clipboard_toast_position: config.ui.toast.clipboard.position,
            palette: crate::ui::palette_from_config(config),
            keybinds,
            mouse_scroll_lines: config.ui.mouse_scroll_lines(),
            right_click_passthrough_modifiers: config.ui.right_click_passthrough_modifiers(),
            redraw_on_focus_gained: config.ui.redraw_on_focus_gained,
            startup_config_diagnostic: None,
        }
    }

    pub(crate) fn with_startup_config_diagnostic(mut self, diagnostic: Option<String>) -> Self {
        self.startup_config_diagnostic = diagnostic;
        self
    }

    pub(super) fn apply_live_config(
        &mut self,
        config: &Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
    ) -> Vec<String> {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        if !invalid_section("keys") {
            match config.live_keybinds_with_diagnostics() {
                Ok((keybinds, keybind_diagnostics)) => {
                    self.keybinds = keybinds;
                    diagnostics.extend(keybind_diagnostics);
                }
                Err(keybind_diagnostics) => diagnostics.extend(
                    keybind_diagnostics
                        .into_iter()
                        .map(|diagnostic| format!("{diagnostic}; kept current keybinds")),
                ),
            }
        }

        if !invalid_section("ui") {
            let ui = &config.ui;
            self.mobile_width_threshold = ui.mobile_width_threshold;
            self.tab_bar_position = ui.tab_bar_position;
            self.agent_panel_sort = ui.agent_panel_sort;
            self.status_indicators = ui.status_indicators;
            self.toast_delivery = ui.toast.delivery;
            self.toast_delay_seconds = ui.toast.delay_seconds;
            self.toast_position = ui.toast.herdr.position;
            self.clipboard_toast_enabled = ui.toast.clipboard.enabled;
            self.clipboard_toast_position = ui.toast.clipboard.position;
            self.mouse_scroll_lines = ui.mouse_scroll_lines();
            self.right_click_passthrough_modifiers = ui.right_click_passthrough_modifiers();
            self.redraw_on_focus_gained = ui.redraw_on_focus_gained;
        }

        if !invalid_section("theme") {
            self.palette = crate::ui::palette_from_config(config);
        }

        diagnostics
    }

    pub(super) fn layout(&self, cols: u16, rows: u16) -> ClientShellLayout {
        if cols <= self.mobile_width_threshold {
            let header_height = rows.min(2);
            return ClientShellLayout {
                tab_bar: Rect::default(),
                mobile_header: Rect::new(0, 0, cols, header_height),
                pane_surface: Rect::new(0, header_height, cols, rows.saturating_sub(header_height)),
            };
        }

        let main = Rect::new(0, 0, cols, rows);
        let show_tab_bar = rows > 1;
        let tab_height = u16::from(show_tab_bar);
        let (tab_bar, pane_surface) = match self.tab_bar_position {
            TabBarPositionConfig::Top => (
                Rect::new(main.x, 0, main.width, tab_height),
                Rect::new(
                    main.x,
                    tab_height,
                    main.width,
                    rows.saturating_sub(tab_height),
                ),
            ),
            TabBarPositionConfig::Bottom => (
                Rect::new(
                    main.x,
                    rows.saturating_sub(tab_height),
                    main.width,
                    tab_height,
                ),
                Rect::new(main.x, 0, main.width, rows.saturating_sub(tab_height)),
            ),
        };

        ClientShellLayout {
            tab_bar,
            mobile_header: Rect::default(),
            pane_surface,
        }
    }

    pub(crate) fn initial_surface_size(&self, cols: u16, rows: u16) -> ClientSurfaceSize {
        let surface = self.layout(cols, rows).pane_surface;
        ClientSurfaceSize {
            cols: surface.width.max(1),
            rows: surface.height.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    #[test]
    fn ui_defaults_to_terminal_colors_and_only_applies_custom_overrides() {
        let mut config = Config::default();
        assert_eq!(
            ClientShellConfig::from_config(&config).palette,
            Palette::terminal()
        );
        config.theme.custom = Some(crate::config::CustomThemeColors {
            accent: Some("#12ab34".into()),
            ..Default::default()
        });
        let mut expected = Palette::terminal();
        expected.accent = ratatui::style::Color::Rgb(0x12, 0xab, 0x34);
        assert_eq!(ClientShellConfig::from_config(&config).palette, expected);
    }

    #[test]
    fn live_override_reload_keeps_invalid_layer_and_can_restore_terminal_defaults() {
        let config: Config = toml::from_str(
            r##"[theme.custom]
accent = "#123456"
"##,
        )
        .unwrap();
        let mut shell = ClientShellConfig::from_config(&config);
        let saved = shell.palette.clone();
        let next: Config = toml::from_str(
            r##"[theme.custom]
accent = "#abcdef"
"##,
        )
        .unwrap();
        shell.apply_live_config(&next, &[], &["theme".into()]);
        assert_eq!(shell.palette, saved);
        shell.apply_live_config(&next, &[], &[]);
        assert_eq!(
            shell.palette.accent,
            ratatui::style::Color::Rgb(0xab, 0xcd, 0xef)
        );
        shell.apply_live_config(&Config::default(), &[], &[]);
        assert_eq!(shell.palette, Palette::terminal());
    }

    #[test]
    fn desktop_tab_bar_defaults_to_bottom() {
        let config: Config = toml::from_str("[ui]").unwrap();
        let layout = ClientShellConfig::from_config(&config).layout(106, 20);
        assert_eq!(layout.tab_bar, Rect::new(0, 19, 106, 1));
        assert_eq!(layout.pane_surface, Rect::new(0, 0, 106, 19));
    }

    #[test]
    fn live_reload_applies_client_owned_sections() {
        let mut shell = ClientShellConfig::from_config(&Config::default());
        let mut next = Config::default();
        next.ui.tab_bar_position = TabBarPositionConfig::Top;
        next.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
        next.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
        next.keys.prefix = crate::config::BindingConfig::one("ctrl+b");

        let diagnostics = shell.apply_live_config(&next, &[], &[]);

        assert!(diagnostics.is_empty());
        assert_eq!(shell.tab_bar_position, TabBarPositionConfig::Top);
        assert_eq!(
            shell.agent_panel_sort,
            crate::config::AgentPanelSortConfig::Priority
        );
        assert_eq!(
            shell.status_indicators,
            crate::config::StatusIndicatorStyle::Symbols
        );
        shell.apply_live_config(
            &Config::default(),
            &[],
            &["ui".to_owned(), "keys".to_owned()],
        );
        assert_eq!(shell.tab_bar_position, TabBarPositionConfig::Top);
        assert_eq!(
            shell.keybinds.prefix,
            vec![(KeyCode::Char('b'), KeyModifiers::CONTROL)]
        );
    }

    #[test]
    fn live_reload_preserves_invalid_client_owned_sections() {
        let mut initial = Config::default();
        initial.ui.tab_bar_position = TabBarPositionConfig::Top;
        initial.keys.prefix = crate::config::BindingConfig::one("ctrl+x");
        let mut shell = ClientShellConfig::from_config(&initial);

        let mut invalid = Config::default();
        invalid.keys.prefix = crate::config::BindingConfig::one("ctrl+a");
        let invalid_sections = vec!["ui".to_owned(), "keys".to_owned()];
        shell.apply_live_config(&invalid, &[], &invalid_sections);

        assert_eq!(shell.tab_bar_position, TabBarPositionConfig::Top);
        assert_eq!(
            shell.keybinds.prefix,
            vec![(KeyCode::Char('x'), KeyModifiers::CONTROL)]
        );
    }
}
