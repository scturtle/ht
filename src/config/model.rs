use std::{collections::BTreeSet, num::NonZeroUsize};

use crossterm::event::KeyModifiers;
use serde::{de, Deserialize, Deserializer, Serialize};

use super::key_fields::key_fields;
use super::keybinds::{format_key_combo, DEFAULT_PREFIX};
use super::{
    BindingConfig, ThemeConfig, DEFAULT_MOBILE_WIDTH_THRESHOLD, DEFAULT_MOUSE_SCROLL_LINES,
    DEFAULT_SCROLLBACK_LIMIT_LINES,
};

pub const MAX_TOAST_DELAY_SECONDS: u64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToastDelivery {
    #[default]
    Off,
    Herdr,
    Terminal,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ToastHerdrPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ToastClipboardPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentPanelSortConfig {
    #[default]
    #[serde(alias = "workspaces")]
    Spaces,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StatusIndicatorStyle {
    #[default]
    Dots,
    Symbols,
}

impl StatusIndicatorStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dots => "dots",
            Self::Symbols => "symbols",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RightClickPassthroughModifierConfig(Option<KeyModifiers>);

impl RightClickPassthroughModifierConfig {
    pub fn modifiers(self) -> Option<KeyModifiers> {
        self.0
    }
}

impl<'de> Deserialize<'de> for RightClickPassthroughModifierConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_right_click_passthrough_modifier(&value)
            .map(Self)
            .ok_or_else(|| {
                de::Error::custom(
                    "right_click_passthrough_modifier must be empty, off, none, disabled, ctrl/control, alt/option, cmd/command/super, meta, hyper, or a + separated combination without shift",
                )
            })
    }
}

fn parse_right_click_passthrough_modifier(value: &str) -> Option<Option<KeyModifiers>> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("disabled")
    {
        return Some(None);
    }

    let mut modifiers = KeyModifiers::empty();
    for token in trimmed.split('+') {
        let token = token.trim().to_ascii_lowercase();
        let modifier = match token.as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "option" => KeyModifiers::ALT,
            "cmd" | "command" | "super" => KeyModifiers::SUPER,
            "meta" => KeyModifiers::META,
            "hyper" => KeyModifiers::HYPER,
            "shift" => return None,
            _ => return None,
        };
        modifiers |= modifier;
    }

    (!modifiers.is_empty()).then_some(Some(modifiers))
}

#[derive(Debug, Clone)]
pub struct ToastConfig {
    pub delivery: ToastDelivery,
    pub delay_seconds: u64,
    pub herdr: HerdrToastConfig,
    pub clipboard: ClipboardToastConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HerdrToastConfig {
    pub position: ToastHerdrPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ClipboardToastConfig {
    pub enabled: bool,
    pub position: ToastClipboardPosition,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Executable used for new interactive panes. Empty means SHELL, then /bin/sh.
    pub default_shell: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigReloadStatus {
    Applied,
    Partial,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigReloadReport {
    pub status: ConfigReloadStatus,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeConfig,
    pub terminal: TerminalConfig,
    pub keys: KeysConfig,
    pub ui: UiConfig,
    pub advanced: AdvancedConfig,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub diagnostics: Vec<String>,
    pub invalid_sections: Vec<String>,
}

macro_rules! define_keys_config {
    ((); $( $section:ident {
        $( $(#[$attr:meta])* $field:ident: $ty:ty = $default:expr => $kind:ident $(($target:ident))?; )*
    } )*) => {
        #[derive(Debug, Clone, Serialize)]
        pub struct KeysConfig {
            /// Prefix key(s) to enter prefix mode (e.g. "ctrl+a", "f12", "esc", or an
            /// array to accept several).
            pub prefix: BindingConfig,
            $($( $(#[$attr])* pub $field: $ty, )*)*
            /// Optional indexed shortcuts expanded over number keys 1-9.
            pub indexed: IndexedKeysConfig,
            #[serde(skip_serializing)]
            pub(crate) user_fields: BTreeSet<&'static str>,
        }

        impl Default for KeysConfig {
            fn default() -> Self {
                Self {
                    prefix: BindingConfig::one(format_key_combo(DEFAULT_PREFIX)),
                    $($( $field: $default, )*)*
                    indexed: IndexedKeysConfig::default(),
                    user_fields: BTreeSet::new(),
                }
            }
        }

        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct KeysConfigOverlay {
            prefix: Option<BindingConfig>,
            // Legacy additional prefixes, merged with the main prefix setting.
            extra_prefixes: Option<BindingConfig>,
            $($( $(#[$attr])* $field: Option<$ty>, )*)*
            indexed: Option<IndexedKeysConfig>,
        }

        impl KeysConfigOverlay {
            fn apply_fields(self, keys: &mut KeysConfig) {
                $($(
                    if let Some(value) = self.$field {
                        keys.$field = value;
                        keys.user_fields.insert(stringify!($field));
                    }
                )*)*
                if let Some(indexed) = self.indexed {
                    keys.indexed = indexed;
                    keys.user_fields.insert("indexed");
                }
            }
        }
    };
}

key_fields!(define_keys_config);

impl<'de> Deserialize<'de> for KeysConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut input = KeysConfigOverlay::deserialize(deserializer)?;
        let mut keys = KeysConfig::default();

        let prefix_was_supplied = input.prefix.is_some() || input.extra_prefixes.is_some();
        let mut prefix_values = Vec::new();
        if let Some(prefix) = input.prefix.take() {
            prefix_values.extend(prefix.into_values());
        }
        if let Some(extra) = input.extra_prefixes.take() {
            prefix_values.extend(extra.into_values());
        }
        if prefix_was_supplied {
            // An explicitly empty list stays empty so prefix validation rejects
            // it and a reload keeps the current keybindings.
            keys.prefix = match prefix_values.len() {
                0 => BindingConfig::Many(Vec::new()),
                1 => BindingConfig::One(prefix_values.remove(0)),
                _ => BindingConfig::Many(prefix_values),
            };
            keys.user_fields.insert("prefix");
        }

        input.apply_fields(&mut keys);
        Ok(keys)
    }
}

impl KeysConfig {
    pub(crate) fn key_field_is_user_configured(&self, field: &str) -> bool {
        self.user_fields.contains(field)
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct IndexedKeysConfig {
    /// Modifier combo for tab shortcuts 1-9. Unset by default.
    pub tabs: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TabBarPositionConfig {
    Top,
    #[default]
    Bottom,
}

/// Configurable presentation preferences. Mouse capture, automatic selection copy,
/// native cursors, direct creation/closing, and inner-only pane dividers are fixed policy.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Terminal width at or below which Herdr uses the mobile single-column layout. Default: 64.
    pub mobile_width_threshold: u16,
    /// Modifier that lets right-click gestures pass through to pane apps. Empty disables it.
    pub right_click_passthrough_modifier: RightClickPassthroughModifierConfig,
    /// Force a full host-terminal redraw when the outer terminal regains focus. Default: true.
    pub redraw_on_focus_gained: bool,
    /// Lines to scroll per mouse wheel notch. Default: 3.
    pub mouse_scroll_lines: Option<NonZeroUsize>,
    /// Desktop tab row placement. Default: bottom.
    pub tab_bar_position: TabBarPositionConfig,
    /// Agent navigation ordering. Saved values are "spaces" or "priority". Default: "spaces".
    pub agent_panel_sort: AgentPanelSortConfig,
    /// Agent status indicator style. Saved values are "dots" or "symbols". Default: "dots".
    pub status_indicators: StatusIndicatorStyle,
    /// Optional visual toast notifications for background workspace events.
    pub toast: ToastConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    /// Physical history rows retained per client pane, approximately (whole-page pruning).
    /// Default: 1000. Zero disables history. Applied when the client starts.
    pub scrollback_limit_lines: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            mobile_width_threshold: DEFAULT_MOBILE_WIDTH_THRESHOLD,
            right_click_passthrough_modifier: RightClickPassthroughModifierConfig::default(),
            redraw_on_focus_gained: true,
            mouse_scroll_lines: None,
            tab_bar_position: TabBarPositionConfig::default(),
            agent_panel_sort: AgentPanelSortConfig::Spaces,
            status_indicators: StatusIndicatorStyle::Dots,
            toast: ToastConfig::default(),
        }
    }
}

impl UiConfig {
    pub fn mouse_scroll_lines(&self) -> usize {
        self.mouse_scroll_lines
            .map(NonZeroUsize::get)
            .unwrap_or(DEFAULT_MOUSE_SCROLL_LINES)
    }

    pub fn right_click_passthrough_modifiers(&self) -> Option<KeyModifiers> {
        self.right_click_passthrough_modifier.modifiers()
    }
}

impl Default for ToastConfig {
    fn default() -> Self {
        Self {
            delivery: ToastDelivery::Off,
            delay_seconds: 1,
            herdr: HerdrToastConfig::default(),
            clipboard: ClipboardToastConfig::default(),
        }
    }
}

impl Default for HerdrToastConfig {
    fn default() -> Self {
        Self {
            position: ToastHerdrPosition::BottomRight,
        }
    }
}

impl Default for ClipboardToastConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: ToastClipboardPosition::BottomCenter,
        }
    }
}

impl<'de> Deserialize<'de> for ToastConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct RawToastConfig {
            delivery: Option<ToastDelivery>,
            enabled: Option<bool>,
            delay_seconds: Option<u64>,
            herdr: HerdrToastConfig,
            clipboard: ClipboardToastConfig,
        }

        let raw = RawToastConfig::deserialize(deserializer)?;
        let legacy_delivery = match raw.enabled {
            Some(true) => ToastDelivery::Herdr,
            Some(false) | None => ToastDelivery::Off,
        };
        let delivery = raw.delivery.unwrap_or(legacy_delivery);
        let default = Self::default();
        let delay_seconds = raw.delay_seconds.unwrap_or(default.delay_seconds);
        if delay_seconds > MAX_TOAST_DELAY_SECONDS {
            return Err(de::Error::custom(format!(
                "ui.toast.delay_seconds must be between 0 and {MAX_TOAST_DELAY_SECONDS}"
            )));
        }
        Ok(Self {
            delivery,
            delay_seconds,
            herdr: raw.herdr,
            clipboard: raw.clipboard,
        })
    }
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            scrollback_limit_lines: DEFAULT_SCROLLBACK_LIMIT_LINES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_panel_sort_config_parses_alias_and_defaults() {
        assert_eq!(
            Config::default().ui.agent_panel_sort,
            AgentPanelSortConfig::Spaces
        );

        let toml = r#"
[ui]
agent_panel_sort = "priority"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.ui.agent_panel_sort, AgentPanelSortConfig::Priority);

        let toml = r#"
[ui]
agent_panel_sort = "workspaces"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.ui.agent_panel_sort, AgentPanelSortConfig::Spaces);
    }

    #[test]
    fn right_click_passthrough_modifier_defaults_off_and_parses() {
        let default_config = Config::default();
        assert_eq!(default_config.ui.right_click_passthrough_modifiers(), None);

        for value in ["", "off", "none", "disabled"] {
            let toml = format!(
                r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
            );
            let config: Config = toml::from_str(&toml).unwrap();
            assert_eq!(
                config.ui.right_click_passthrough_modifiers(),
                None,
                "value {value:?} should disable passthrough"
            );
        }

        for (value, expected) in [
            ("ctrl", KeyModifiers::CONTROL),
            ("control", KeyModifiers::CONTROL),
            ("alt", KeyModifiers::ALT),
            ("option", KeyModifiers::ALT),
            ("cmd", KeyModifiers::SUPER),
            ("command", KeyModifiers::SUPER),
            ("super", KeyModifiers::SUPER),
            ("meta", KeyModifiers::META),
            ("hyper", KeyModifiers::HYPER),
        ] {
            let toml = format!(
                r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
            );
            let config: Config = toml::from_str(&toml).unwrap();
            assert_eq!(
                config.ui.right_click_passthrough_modifiers(),
                Some(expected),
                "value {value:?} should parse"
            );
        }

        let toml = r#"
[ui]
right_click_passthrough_modifier = "cmd+alt"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config.ui.right_click_passthrough_modifiers(),
            Some(KeyModifiers::SUPER | KeyModifiers::ALT)
        );
    }

    #[test]
    fn right_click_passthrough_modifier_rejects_shift() {
        for value in ["shift", "shift+ctrl", "ctrl+", "ctrl++alt", "banana"] {
            let toml = format!(
                r#"
[ui]
right_click_passthrough_modifier = "{value}"
"#
            );
            assert!(
                toml::from_str::<Config>(&toml).is_err(),
                "value {value:?} should be rejected"
            );
        }
    }

    #[test]
    fn mouse_scroll_lines_rejects_zero() {
        let toml = r#"
[ui]
mouse_scroll_lines = 0
"#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn toast_config_legacy_enabled_true_maps_to_herdr() {
        let toml = r#"
[ui.toast]
enabled = true
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.ui.toast.delivery, ToastDelivery::Herdr);
    }

    #[test]
    fn toast_config_legacy_enabled_false_maps_to_off() {
        let toml = r#"
[ui.toast]
enabled = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.ui.toast.delivery, ToastDelivery::Off);
    }

    #[test]
    fn toast_config_delivery_wins_over_legacy_enabled() {
        let toml = r#"
[ui.toast]
enabled = true
delivery = "terminal"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.ui.toast.delivery, ToastDelivery::Terminal);
    }

    #[test]
    fn toast_config_rejects_unbounded_delay() {
        let toml = format!(
            r#"
[ui.toast]
delay_seconds = {}
"#,
            MAX_TOAST_DELAY_SECONDS + 1
        );

        let error = toml::from_str::<Config>(&toml).unwrap_err().to_string();

        assert!(error.contains("ui.toast.delay_seconds must be between 0 and 3600"));
    }

    #[test]
    fn kitty_graphics_setting_is_ignored() {
        let config: Config = toml::from_str(
            r#"
[terminal]
kitty_graphics = false
"#,
        )
        .unwrap();
        assert_eq!(config.terminal.default_shell, "");
    }

    #[test]
    fn scrollback_line_limit_accepts_zero_but_rejects_negative_or_overflowing_counts() {
        let config: Config = toml::from_str("[advanced]\nscrollback_limit_lines = 0").unwrap();
        assert_eq!(config.advanced.scrollback_limit_lines, 0);
        for count in ["-1", "4294967296", "\"1000\""] {
            assert!(toml::from_str::<Config>(&format!(
                "[advanced]\nscrollback_limit_lines = {count}"
            ))
            .is_err());
        }
    }
}
