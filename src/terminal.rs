//! Shared terminal-theme data and pure color/appearance escape-sequence codecs.
//! Host I/O belongs to client::host_terminal; pane state belongs to pane::terminal.

use bytes::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAppearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalTheme {
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub palette: [Option<RgbColor>; 256],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            palette: [None; 256],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultColorKind {
    Foreground,
    Background,
}

impl TerminalTheme {
    pub fn with_palette_color(mut self, index: u8, color: RgbColor) -> Self {
        self.palette[usize::from(index)] = Some(color);
        self
    }

    #[cfg(test)]
    pub fn is_empty(self) -> bool {
        self.foreground.is_none() && self.background.is_none()
    }
}

pub const HOST_COLOR_QUERY_SEQUENCE: &str = "\x1b]10;?\x1b\\\x1b]11;?\x1b\\";
pub const HOST_COLOR_SCHEME_QUERY_SEQUENCE: &str = "\x1b[?996n";
pub const HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE: &str = "\x1b[?2031h";
pub const HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE: &str = "\x1b[?2031l";

pub(crate) const COLOR_SCHEME_DARK_REPORT: &[u8] = b"\x1b[?997;1n";
pub(crate) const COLOR_SCHEME_LIGHT_REPORT: &[u8] = b"\x1b[?997;2n";

pub(crate) const fn color_scheme_report(appearance: HostAppearance) -> &'static [u8] {
    match appearance {
        HostAppearance::Dark => COLOR_SCHEME_DARK_REPORT,
        HostAppearance::Light => COLOR_SCHEME_LIGHT_REPORT,
    }
}

pub(crate) fn parse_color_scheme_report(buffer: &[u8]) -> Option<HostAppearance> {
    match buffer {
        COLOR_SCHEME_DARK_REPORT => Some(HostAppearance::Dark),
        COLOR_SCHEME_LIGHT_REPORT => Some(HostAppearance::Light),
        _ => None,
    }
}

pub fn parse_default_color_response(sequence: &str) -> Option<(DefaultColorKind, RgbColor)> {
    let body = sequence.strip_prefix("\x1b]")?;
    let body = body
        .strip_suffix("\x1b\\")
        .or_else(|| body.strip_suffix('\u{7}'))?;
    let (command, value) = body.split_once(';')?;
    let kind = match command {
        "10" => DefaultColorKind::Foreground,
        "11" => DefaultColorKind::Background,
        _ => return None,
    };
    Some((kind, parse_rgb_color(value)?))
}

pub fn parse_palette_color_response(sequence: &str) -> Option<(u8, RgbColor)> {
    let body = sequence.strip_prefix("\x1b]4;")?;
    let body = body
        .strip_suffix("\x1b\\")
        .or_else(|| body.strip_suffix('\u{7}'))?;
    let (index, value) = body.split_once(';')?;
    Some((index.parse().ok()?, parse_rgb_color(value)?))
}

pub fn osc_set_default_color_sequence(kind: DefaultColorKind, color: RgbColor) -> String {
    let command = match kind {
        DefaultColorKind::Foreground => 10,
        DefaultColorKind::Background => 11,
    };
    format!(
        "\x1b]{command};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
        color.r, color.g, color.b
    )
}

pub fn osc_reset_default_color_sequence(kind: DefaultColorKind) -> &'static str {
    match kind {
        DefaultColorKind::Foreground => "\x1b]110\x1b\\",
        DefaultColorKind::Background => "\x1b]111\x1b\\",
    }
}

fn parse_rgb_color(value: &str) -> Option<RgbColor> {
    if let Some(rgb) = value.strip_prefix("rgb:") {
        let mut parts = rgb.split('/');
        let color = RgbColor {
            r: parse_hex_component(parts.next()?)?,
            g: parse_hex_component(parts.next()?)?,
            b: parse_hex_component(parts.next()?)?,
        };
        return parts.next().is_none().then_some(color);
    }

    if let Some(hex) = value.strip_prefix('#') {
        let digits = hex.len() / 3;
        if !hex.is_ascii() || !matches!(digits, 1..=4) || hex.len() != digits * 3 {
            return None;
        }
        return Some(RgbColor {
            r: parse_hex_component(&hex[..digits])?,
            g: parse_hex_component(&hex[digits..digits * 2])?,
            b: parse_hex_component(&hex[digits * 2..])?,
        });
    }

    None
}

fn parse_hex_component(component: &str) -> Option<u8> {
    if component.is_empty()
        || component.len() > 4
        || !component.bytes().all(|ch| ch.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let max = (1u32 << (component.len() * 4)) - 1;
    Some(((value * 255 + (max / 2)) / max) as u8)
}

pub(crate) fn osc_rgb_response(command: &str, r: u8, g: u8, b: u8) -> Bytes {
    let r = u16::from(r) * 257;
    let g = u16::from(g) * 257;
    let b = u16::from(b) * 257;
    Bytes::from(format!("\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}\x1b\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_color_set_sequences_round_trip() {
        for kind in [DefaultColorKind::Foreground, DefaultColorKind::Background] {
            for color in [
                RgbColor::default(),
                RgbColor {
                    r: 12,
                    g: 128,
                    b: 255,
                },
                RgbColor {
                    r: 255,
                    g: 255,
                    b: 255,
                },
            ] {
                let sequence = osc_set_default_color_sequence(kind, color);
                assert_eq!(parse_default_color_response(&sequence), Some((kind, color)));
            }
        }
        assert_eq!(
            osc_reset_default_color_sequence(DefaultColorKind::Foreground),
            "\x1b]110\x1b\\"
        );
        assert_eq!(
            osc_reset_default_color_sequence(DefaultColorKind::Background),
            "\x1b]111\x1b\\"
        );
    }

    #[test]
    fn color_responses_accept_supported_precisions_and_terminators() {
        for value in [
            "rgb:8/4/2",
            "#842",
            "rgb:88/44/22",
            "#884422",
            "rgb:888/444/222",
            "#888444222",
            "rgb:8888/4444/2222",
            "#888844442222",
        ] {
            let color = RgbColor {
                r: 136,
                g: 68,
                b: 34,
            };
            for end in ["\x07", "\x1b\\"] {
                assert_eq!(
                    parse_default_color_response(&format!("\x1b]11;{value}{end}")),
                    Some((DefaultColorKind::Background, color)),
                );
                assert_eq!(
                    parse_palette_color_response(&format!("\x1b]4;255;{value}{end}")),
                    Some((255, color)),
                );
            }
        }
    }

    #[test]
    fn color_responses_reject_malformed_values_and_out_of_range_indices() {
        for value in [
            "rgb:/0/0",
            "rgb:0/0",
            "rgb:0/0/0/0",
            "rgb:00000/0/0",
            "rgb:g/0/0",
            "#12",
            "#1234",
            "#１２３",
        ] {
            assert_eq!(
                parse_default_color_response(&format!("\x1b]10;{value}\x07")),
                None
            );
            assert_eq!(
                parse_palette_color_response(&format!("\x1b]4;0;{value}\x07")),
                None
            );
        }
        assert_eq!(parse_default_color_response("\x1b]12;rgb:0/0/0\x07"), None);
        assert_eq!(parse_default_color_response("\x1b]10;rgb:0/0/0"), None);
        assert_eq!(
            parse_palette_color_response("\x1b]4;256;rgb:0/0/0\x07"),
            None
        );
        assert_eq!(
            parse_palette_color_response("\x1b]4;-1;rgb:0/0/0\x07"),
            None
        );
    }

    #[test]
    fn color_replies_keep_sixteen_bit_components_separate_from_set_sequences() {
        let color = RgbColor {
            r: 12,
            g: 128,
            b: 255,
        };
        assert_eq!(
            osc_set_default_color_sequence(DefaultColorKind::Foreground, color),
            "\x1b]10;rgb:0c/80/ff\x1b\\",
        );
        assert_eq!(
            osc_rgb_response("10", 12, 128, 255).as_ref(),
            b"\x1b]10;rgb:0c0c/8080/ffff\x1b\\"
        );
        assert_eq!(
            osc_rgb_response("4;255", 12, 128, 255).as_ref(),
            b"\x1b]4;255;rgb:0c0c/8080/ffff\x1b\\"
        );
    }

    #[test]
    fn appearance_queries_modes_and_reports_keep_their_wire_encoding() {
        assert_eq!(HOST_COLOR_QUERY_SEQUENCE, "\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        assert_eq!(HOST_COLOR_SCHEME_QUERY_SEQUENCE, "\x1b[?996n");
        assert_eq!(HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE, "\x1b[?2031h");
        assert_eq!(HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE, "\x1b[?2031l");
        for (appearance, expected) in [
            (HostAppearance::Dark, b"\x1b[?997;1n"),
            (HostAppearance::Light, b"\x1b[?997;2n"),
        ] {
            assert_eq!(color_scheme_report(appearance), expected);
            assert_eq!(parse_color_scheme_report(expected), Some(appearance));
        }
        assert_eq!(parse_color_scheme_report(b"\x1b[?997;3n"), None);
        assert_eq!(parse_color_scheme_report(b"\x1b[?997;1"), None);
    }
}
