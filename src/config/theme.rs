use serde::Deserialize;
use tracing::warn;

/// One optional override layer on the host terminal's ANSI/default colors.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub custom: Option<CustomThemeColors>,
}

/// Per-token color overrides. All fields optional — only set what you want to change.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CustomThemeColors {
    pub accent: Option<String>,
    pub panel_bg: Option<String>,
    pub active_row_bg: Option<String>,
    pub selection_bg: Option<String>,
    pub surface0: Option<String>,
    pub surface1: Option<String>,
    pub surface_dim: Option<String>,
    pub overlay0: Option<String>,
    pub overlay1: Option<String>,
    pub text: Option<String>,
    pub subtext0: Option<String>,
    pub mauve: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub red: Option<String>,
    pub blue: Option<String>,
    pub teal: Option<String>,
    pub peach: Option<String>,
}

/// Parse a color string into a ratatui Color.
/// Supports: hex (#rrggbb, #rgb), named colors, rgb(r,g,b), and reset aliases.
pub fn parse_color(s: &str) -> ratatui::style::Color {
    use ratatui::style::Color;
    let s = s.trim().to_lowercase();

    match s.as_str() {
        "reset" | "default" | "none" | "transparent" => return Color::Reset,
        _ => {}
    }

    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        } else if hex.len() == 3 {
            let chars: Vec<u8> = hex
                .chars()
                .filter_map(|c| u8::from_str_radix(&c.to_string(), 16).ok())
                .collect();
            if chars.len() == 3 {
                return Color::Rgb(chars[0] * 17, chars[1] * 17, chars[2] * 17);
            }
        }
    }

    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                parts[0].trim().parse::<u8>(),
                parts[1].trim().parse::<u8>(),
                parts[2].trim().parse::<u8>(),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }

    match s.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => {
            warn!(color = s, "unknown color, defaulting to cyan");
            Color::Cyan
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parse_color_accepts_reset_aliases() {
        use ratatui::style::Color;

        for value in ["reset", "default", "none", "transparent"] {
            assert_eq!(parse_color(value), Color::Reset, "value: {value}");
        }
    }

    #[test]
    fn theme_custom_overrides_parse() {
        let toml = r##"
[theme]
name = "nord"

[theme.custom]
panel_bg = "#1e1e2e"
active_row_bg = "#313244"
selection_bg = "#45475a"
accent = "#ff79c6"
red = "rgb(255, 85, 85)"
"##;
        let config: Config = toml::from_str(toml).unwrap();
        let custom = config.theme.custom.as_ref().unwrap();
        assert_eq!(custom.panel_bg.as_deref(), Some("#1e1e2e"));
        assert_eq!(custom.active_row_bg.as_deref(), Some("#313244"));
        assert_eq!(custom.selection_bg.as_deref(), Some("#45475a"));
        assert_eq!(custom.accent.as_deref(), Some("#ff79c6"));
        assert_eq!(custom.red.as_deref(), Some("rgb(255, 85, 85)"));
        assert!(custom.green.is_none());
    }

    #[test]
    fn theme_defaults_when_missing() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.theme.custom.is_none());
    }
}
