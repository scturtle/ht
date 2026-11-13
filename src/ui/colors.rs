//! Resolve the single optional UI override layer over host terminal colors.
use super::Palette;
use crate::config::Config;

pub(crate) fn palette_from_config(config: &Config) -> Palette {
    let palette = Palette::terminal();
    match &config.theme.custom {
        Some(custom) => palette.with_overrides(custom),
        None => palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn only_explicit_tokens_override_the_terminal_palette() {
        let config: Config = toml::from_str(
            r##"
[theme.custom]
accent = "#123456"
panel_bg = "reset"
red = "rgb(10,20,30)"
"##,
        )
        .unwrap();
        let mut expected = Palette::terminal();
        expected.accent = Color::Rgb(0x12, 0x34, 0x56);
        expected.red = Color::Rgb(10, 20, 30);
        assert_eq!(palette_from_config(&config), expected);
        assert_eq!(palette_from_config(&Config::default()), Palette::terminal());
    }
}
