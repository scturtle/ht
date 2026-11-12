//! Pure UI theme resolution. Host color protocol parsing stays in terminal_theme.
use super::Palette;
use crate::{config::Config, terminal_theme::HostAppearance};

/// Client-owned theme settings, independent of PTYs and shared Core state.
#[derive(Debug, Clone)]
pub(crate) struct ThemeRuntimeConfig {
    pub manual_name: String,
    pub dark_name: String,
    pub light_name: String,
    pub auto_switch: bool,
    pub custom: Option<crate::config::CustomThemeColors>,
    pub legacy_accent: Option<String>,
}

fn sibling_theme_names(name: &str) -> (String, String) {
    let pair = match crate::config::canonical_theme_name(name) {
        Some("catppuccin" | "catppuccin-latte") => ("catppuccin", "catppuccin-latte"),
        Some("tokyo-night" | "tokyo-night-day") => ("tokyo-night", "tokyo-night-day"),
        Some("gruvbox" | "gruvbox-light") => ("gruvbox", "gruvbox-light"),
        Some("one-dark" | "one-light") => ("one-dark", "one-light"),
        Some("solarized" | "solarized-light") => ("solarized", "solarized-light"),
        Some("kanagawa" | "kanagawa-lotus") => ("kanagawa", "kanagawa-lotus"),
        Some("rose-pine" | "rose-pine-dawn") => ("rose-pine", "rose-pine-dawn"),
        _ => (name, name),
    };
    (pair.0.to_owned(), pair.1.to_owned())
}

impl ThemeRuntimeConfig {
    pub(crate) fn from_config(config: &Config) -> Self {
        let manual_name = config
            .theme
            .name
            .clone()
            .unwrap_or_else(|| "catppuccin".to_owned());
        let (default_dark, default_light) = sibling_theme_names(&manual_name);
        Self {
            manual_name,
            dark_name: config.theme.dark_name.clone().unwrap_or(default_dark),
            light_name: config.theme.light_name.clone().unwrap_or(default_light),
            auto_switch: config.theme.auto_switch,
            custom: config.theme.custom.clone(),
            legacy_accent: (config.ui.accent != "cyan"
                && config
                    .theme
                    .custom
                    .as_ref()
                    .and_then(|c| c.accent.as_ref())
                    .is_none())
            .then(|| config.ui.accent.clone()),
        }
    }

    pub(crate) fn palette(&self) -> Palette {
        self.palette_for_appearance(HostAppearance::Dark)
    }

    pub(crate) fn palette_for_theme(&self, name: &str) -> Palette {
        self.resolve(name, "catppuccin", None)
    }

    pub(crate) fn palette_for_appearance(&self, appearance: HostAppearance) -> Palette {
        let (name, fallback, mode_custom) = if self.auto_switch {
            match appearance {
                HostAppearance::Dark => (
                    &self.dark_name,
                    "catppuccin",
                    self.custom.as_ref().and_then(|custom| custom.dark.as_ref()),
                ),
                HostAppearance::Light => (
                    &self.light_name,
                    "catppuccin-latte",
                    self.custom
                        .as_ref()
                        .and_then(|custom| custom.light.as_ref()),
                ),
            }
        } else {
            (&self.manual_name, "catppuccin", None)
        };
        self.resolve(name, fallback, mode_custom)
    }

    fn resolve(
        &self,
        name: &str,
        fallback: &str,
        mode_custom: Option<&crate::config::ModeThemeColors>,
    ) -> Palette {
        let mut palette = Palette::from_name(name).unwrap_or_else(|| {
            tracing::warn!(theme = name, fallback, "unknown theme, falling back");
            Palette::from_name(fallback).unwrap_or_else(Palette::catppuccin)
        });
        if let Some(custom) = &self.custom {
            palette = palette.with_overrides(custom);
        }
        if let Some(accent) = &self.legacy_accent {
            palette.accent = crate::config::parse_color(accent);
        }
        if let Some(custom) = mode_custom {
            palette = palette.with_mode_overrides(custom);
        }
        palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn every_configured_theme_and_alias_resolves() {
        for name in crate::config::THEME_NAMES {
            assert!(Palette::from_name(name).is_some(), "{name}");
        }
        for (alias, dark, light) in [
            ("Catppuccin_Mocha", "catppuccin", "catppuccin-latte"),
            ("light", "catppuccin", "catppuccin-latte"),
            ("TokyoNight Day", "tokyo-night", "tokyo-night-day"),
            ("gruvbox-dark", "gruvbox", "gruvbox-light"),
            ("OneLight", "one-dark", "one-light"),
            ("solarized-light", "solarized", "solarized-light"),
            ("lotus", "kanagawa", "kanagawa-lotus"),
            ("RosePine_Dawn", "rose-pine", "rose-pine-dawn"),
            ("terminal", "terminal", "terminal"),
            ("unknown", "unknown", "unknown"),
        ] {
            assert_eq!(sibling_theme_names(alias), (dark.into(), light.into()));
        }
    }

    #[test]
    fn manual_auto_and_unknown_theme_fallbacks_are_preserved() {
        let mut config = Config::default();
        config.theme.name = Some("nord".into());
        let manual = ThemeRuntimeConfig::from_config(&config);
        assert_eq!(manual.palette(), Palette::nord());
        assert_eq!(
            manual.palette_for_appearance(HostAppearance::Light),
            Palette::nord()
        );
        assert_eq!(manual.palette_for_theme("unknown"), Palette::catppuccin());
        config.theme.auto_switch = true;
        config.theme.dark_name = Some("unknown".into());
        config.theme.light_name = Some("unknown".into());
        let automatic = ThemeRuntimeConfig::from_config(&config);
        assert_eq!(automatic.palette(), Palette::catppuccin());
        assert_eq!(
            automatic.palette_for_appearance(HostAppearance::Light),
            Palette::catppuccin_latte()
        );
    }

    #[test]
    fn global_legacy_and_appearance_overrides_keep_their_precedence() {
        let mut config = Config::default();
        config.ui.accent = "red".into();
        config.theme.auto_switch = true;
        config.theme.custom = Some(crate::config::CustomThemeColors {
            text: Some("green".into()),
            light: Some(crate::config::ModeThemeColors {
                accent: Some("blue".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let theme = ThemeRuntimeConfig::from_config(&config);
        assert_eq!(theme.palette().accent, Color::Red);
        assert_eq!(theme.palette().text, Color::Green);
        assert_eq!(
            theme.palette_for_appearance(HostAppearance::Light).accent,
            Color::Blue
        );
        config.theme.custom.as_mut().unwrap().accent = Some("yellow".into());
        let theme = ThemeRuntimeConfig::from_config(&config);
        assert_eq!(theme.legacy_accent, None);
        assert_eq!(theme.palette().accent, Color::Yellow);
        assert_eq!(
            theme.palette_for_appearance(HostAppearance::Light).accent,
            Color::Blue
        );
        assert_eq!(theme.palette_for_theme("nord").accent, Color::Yellow);
    }
}
