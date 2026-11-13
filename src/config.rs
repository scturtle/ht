use crossterm::event::{KeyCode, KeyModifiers};

mod io;
mod keybinds;
mod model;
mod sound;
mod tab_bar;
mod theme;
mod window_title;
mod write;

pub use self::{
    io::{
        config_diagnostic_summary, config_dir, config_path, load_live_config,
        remove_keybinding_config_sections, remove_section_key, state_dir, upsert_section_bool,
        upsert_section_value,
    },
    keybinds::{
        format_prefix_combos, normalize_key_combo, terminal_key_matches_combo, ActionKeybinds,
        BindingConfig, IndexedKeybind, KeyCombo, Keybinds, LiveKeybindConfig,
    },
    model::{
        AdvancedConfig, AgentPanelSortConfig, Config, HostCursorModeConfig, PaneBordersConfig,
        StatusIndicatorStyle, TabBarPositionConfig, ToastClipboardPosition, ToastDelivery,
        ToastHerdrPosition,
    },
    sound::SoundConfig,
    tab_bar::TabBarRightEntryConfig,
    theme::{parse_color, CustomThemeColors, ModeThemeColors, ThemeConfig, THEME_NAMES},
};

pub(crate) use self::keybinds::parse_key_combo;
pub(crate) use self::write::{write_edit, ConfigEdit};
pub(crate) use self::{
    tab_bar::tab_bar_right_diagnostics, theme::canonical_theme_name,
    window_title::window_title_diagnostics,
};

pub const CONFIG_PATH_ENV_VAR: &str = "HERDR_CONFIG_PATH";

pub const DEFAULT_SCROLLBACK_LIMIT_LINES: u32 = 1000;
pub const DEFAULT_MOUSE_SCROLL_LINES: usize = 3;
pub const DEFAULT_MOBILE_WIDTH_THRESHOLD: u16 = 64;

#[cfg(test)]
pub(crate) fn app_dir_name() -> &'static str {
    io::app_dir_name()
}

#[cfg(test)]
pub(crate) fn test_config_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

impl Config {
    pub fn kitty_graphics_enabled(&self) -> bool {
        self.terminal
            .kitty_graphics
            .or(self.experimental.kitty_graphics)
            .unwrap_or(true)
    }

    pub fn prefix_keys(&self) -> Vec<(KeyCode, KeyModifiers)> {
        self.validated_keybinds().1
    }

    /// Parsed keybinds for Herdr actions.
    pub fn keybinds(&self) -> Keybinds {
        self.validated_keybinds().3
    }

    pub fn collect_diagnostics(&self) -> Vec<String> {
        let (prefix_diag, _, keybind_diags, _) = self.validated_keybinds();
        prefix_diag
            .into_iter()
            .chain(keybind_diags)
            .chain(self.remote_image_paste_key().err())
            .chain(self.theme.diagnostics())
            .chain(self.ui.sound.diagnostics())
            .chain(tab_bar_right_diagnostics(&self.ui.tab_bar_right))
            .chain(window_title_diagnostics(&self.ui.window_title))
            .collect()
    }

    pub(crate) fn remote_image_paste_key(&self) -> Result<Option<(KeyCode, KeyModifiers)>, String> {
        let raw = self.keys.remote_image_paste.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        parse_key_combo(raw).map(Some).ok_or_else(|| {
            format!("invalid keybinding: keys.remote_image_paste = {raw:?}; disabling binding")
        })
    }

    pub(crate) fn live_keybinds_with_diagnostics(
        &self,
    ) -> Result<(LiveKeybindConfig, Vec<String>), Vec<String>> {
        let (prefix_diag, prefix, keybind_diags, keybinds) = self.validated_keybinds();
        if let Some(prefix_diag) = prefix_diag {
            Err(std::iter::once(prefix_diag).chain(keybind_diags).collect())
        } else {
            Ok((LiveKeybindConfig { prefix, keybinds }, keybind_diags))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_keymap_preserves_plus_and_multiple_prefixes() {
        let config: Config = toml::from_str(
            "[keys]\nprefix = [\"ctrl+plus\", \"alt+plus\"]\nnew_tab = \"prefix+plus\"",
        )
        .unwrap();
        let (live, diagnostics) = config.live_keybinds_with_diagnostics().unwrap();
        assert!(diagnostics.is_empty());
        assert_eq!(live.prefix, config.prefix_keys());
        assert_eq!(live.prefix.len(), 2);
        assert_eq!(live.prefix[0], (KeyCode::Char('+'), KeyModifiers::CONTROL));
        assert_eq!(live.prefix[1], (KeyCode::Char('+'), KeyModifiers::ALT));
        assert_eq!(
            live.keybinds.new_tab.bindings,
            config.keybinds().new_tab.bindings
        );
        let legacy: Config = toml::from_str(
            "[keys]\nprefix = 'ctrl+plus'\nextra_prefixes = ['alt+plus']\nnew_tab = 'prefix+plus'",
        )
        .unwrap();
        assert_eq!(legacy.prefix_keys(), live.prefix);
        assert_eq!(
            legacy.keybinds().new_tab.bindings,
            live.keybinds.new_tab.bindings
        );
    }

    #[test]
    fn invalid_local_prefix_falls_back_at_startup_but_rejects_reload() {
        let config: Config = toml::from_str("[keys]\nprefix = \"ctrl+\"").unwrap();
        assert_eq!(config.prefix_keys(), Config::default().prefix_keys());
        assert!(config.live_keybinds_with_diagnostics().is_err());
    }

    #[test]
    fn local_user_bindings_keep_displaced_defaults_disabled() {
        let config: Config = toml::from_str("[keys]\nzoom = \"prefix+?\"").unwrap();
        assert!(config.keys.key_field_is_user_configured("zoom"));
        assert!(!config.keys.key_field_is_user_configured("help"));
        assert!(config
            .keybinds()
            .zoom
            .bindings
            .iter()
            .any(|b| b.label == "prefix+?"));
        assert!(config.keybinds().help.bindings.is_empty());

        let prefix: Config = toml::from_str("[keys]\nprefix = \"n\"").unwrap();
        assert!(prefix.keybinds().next_tab.bindings.is_empty());
    }

    #[test]
    fn local_legacy_indexed_bindings_keep_valid_aliases_and_reject_invalid_modifiers() {
        let config: Config = toml::from_str("[keys.indexed]\ntabs = \"ctrl\"").unwrap();
        let keys = config.keybinds();
        assert_eq!(keys.switch_tab.len(), 9);
        assert!(keys.switch_tab.iter().all(|b| b.label.starts_with("ctrl+")));
        let invalid: Config = toml::from_str("[keys.indexed]\ntabs = \"bogus\"").unwrap();
        assert!(invalid.keybinds().switch_tab.is_empty());
    }

    #[test]
    fn remote_image_paste_key_defaults_to_ctrl_v() {
        let config = Config::default();
        assert_eq!(
            config.remote_image_paste_key().unwrap(),
            Some((KeyCode::Char('v'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn remote_image_paste_key_can_be_disabled() {
        let config: Config = toml::from_str("[keys]\nremote_image_paste = ''\n").unwrap();
        assert_eq!(config.remote_image_paste_key().unwrap(), None);
    }

    #[test]
    fn ui_host_cursor_defaults_to_auto_and_parses_overrides() {
        let default_config = Config::default();
        assert_eq!(default_config.ui.host_cursor, HostCursorModeConfig::Auto);

        let native: Config = toml::from_str("[ui]\nhost_cursor = 'native'\n").unwrap();
        assert_eq!(native.ui.host_cursor, HostCursorModeConfig::Native);

        let drawn: Config = toml::from_str("[ui]\nhost_cursor = 'drawn'\n").unwrap();
        assert_eq!(drawn.ui.host_cursor, HostCursorModeConfig::Drawn);
    }
}
