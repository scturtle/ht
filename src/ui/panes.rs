use super::widgets::panel_contrast_fg;
use crate::layout::PaneInfo;
use crate::ui::Palette;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders},
};
pub(crate) fn pane_inner_rect(area: Rect, borders: Borders) -> Rect {
    if borders.is_empty() {
        area
    } else {
        Block::default().borders(borders).inner(area)
    }
}

/// Original zoom geometry, without runtime access or scrollbar mutation.
pub(crate) fn zoomed_pane_info(
    id: crate::layout::PaneId,
    area: Rect,
    pane_borders: crate::config::PaneBordersConfig,
    pane_outer_borders: bool,
    multi_pane: bool,
) -> PaneInfo {
    let borders = if pane_borders.shows_borders(multi_pane) && pane_outer_borders {
        Borders::ALL
    } else {
        Borders::NONE
    };
    PaneInfo {
        id,
        rect: area,
        inner_rect: pane_inner_rect(area, borders),
        scrollbar_rect: None,
        borders,
        is_focused: true,
    }
}

fn ranges_overlap(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    a_start < b_start.saturating_add(b_len) && b_start < a_start.saturating_add(a_len)
}

fn pane_to_right<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let right = info.rect.x.saturating_add(info.rect.width);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.x == right
            && ranges_overlap(
                info.rect.y,
                info.rect.height,
                other.rect.y,
                other.rect.height,
            )
    })
}

fn pane_below<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let bottom = info.rect.y.saturating_add(info.rect.height);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.y == bottom
            && ranges_overlap(info.rect.x, info.rect.width, other.rect.x, other.rect.width)
    })
}

fn shrink_for_one_cell_gap(size: u16) -> u16 {
    if size > 1 {
        size - 1
    } else {
        size
    }
}

pub(crate) fn apply_pane_chrome(
    panes: Vec<PaneInfo>,
    pane_borders: crate::config::PaneBordersConfig,
    pane_gaps: bool,
    pane_outer_borders: bool,
) -> Vec<PaneInfo> {
    let multi_pane = panes.len() > 1;
    let bordered = pane_borders.shows_borders(multi_pane);
    let outer_left = panes.iter().map(|info| info.rect.x).min().unwrap_or(0);
    let outer_top = panes.iter().map(|info| info.rect.y).min().unwrap_or(0);
    let outer_right = panes
        .iter()
        .map(|info| info.rect.x.saturating_add(info.rect.width))
        .max()
        .unwrap_or(0);
    let outer_bottom = panes
        .iter()
        .map(|info| info.rect.y.saturating_add(info.rect.height))
        .max()
        .unwrap_or(0);
    panes
        .iter()
        .cloned()
        .map(|mut info| {
            let right_neighbor = multi_pane.then(|| pane_to_right(&info, &panes)).flatten();
            let below_neighbor = multi_pane.then(|| pane_below(&info, &panes)).flatten();

            if multi_pane && pane_gaps && !pane_borders.draws_borders() {
                if right_neighbor.is_some() {
                    info.rect.width = shrink_for_one_cell_gap(info.rect.width);
                }
                if below_neighbor.is_some() {
                    info.rect.height = shrink_for_one_cell_gap(info.rect.height);
                }
            }

            info.borders = if !bordered {
                Borders::NONE
            } else {
                let mut borders = Borders::ALL;
                if !pane_gaps {
                    if right_neighbor.is_some() {
                        borders.remove(Borders::RIGHT);
                    }
                    if below_neighbor.is_some() {
                        borders.remove(Borders::BOTTOM);
                    }
                }
                if !pane_outer_borders {
                    if info.rect.x == outer_left {
                        borders.remove(Borders::LEFT);
                    }
                    if info.rect.y == outer_top {
                        borders.remove(Borders::TOP);
                    }
                    if info.rect.x.saturating_add(info.rect.width) == outer_right {
                        borders.remove(Borders::RIGHT);
                    }
                    if info.rect.y.saturating_add(info.rect.height) == outer_bottom {
                        borders.remove(Borders::BOTTOM);
                    }
                }
                borders
            };
            info
        })
        .collect()
}

pub(crate) fn render_selection_highlight<P: PartialEq>(
    selection: Option<&crate::selection::Selection<P>>,
    buffer: &mut Buffer,
    pane_id: &P,
    inner: Rect,
    scroll_metrics: Option<crate::pane::ScrollMetrics>,
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) {
    let Some(selection) =
        selection.filter(|selection| selection.is_visible() && &selection.pane_id == pane_id)
    else {
        return;
    };
    let style = automatic_selection_style(p, host_theme);
    for y in 0..inner.height {
        for x in 0..inner.width {
            if selection.contains(y, x, scroll_metrics) {
                buffer[(inner.x + x, inner.y + y)].set_style(style);
            }
        }
    }
}

type Rgb = (u8, u8, u8);

fn automatic_selection_style(
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) -> Style {
    let bg = automatic_selection_bg(p, host_theme);
    Style::reset().fg(selection_fg_for_bg(bg, p)).bg(bg)
}

fn automatic_selection_bg(p: &Palette, host_theme: crate::terminal_theme::TerminalTheme) -> Color {
    let fallback = selection_palette_background(p);
    let Some(background) = host_theme
        .background
        .map(|color| (color.r, color.g, color.b))
        .or(match fallback {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        })
    else {
        return fallback;
    };

    let target = if relative_luminance(background) < 0.5 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    let selected = mix_rgb(background, target, 0.28);
    Color::Rgb(selected.0, selected.1, selected.2)
}

fn selection_palette_background(p: &Palette) -> Color {
    if p.panel_bg == Color::Reset {
        p.surface_dim
    } else {
        p.panel_bg
    }
}

fn selection_fg_for_bg(bg: Color, p: &Palette) -> Color {
    if let Color::Rgb(r, g, b) = bg {
        let luminance = relative_luminance((r, g, b));
        let black_contrast = (luminance + 0.05) / 0.05;
        let white_contrast = 1.05 / (luminance + 0.05);
        return if black_contrast > white_contrast {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        };
    }

    color_to_rgb(bg)
        .map(|bg| {
            if relative_luminance(bg) < 0.5 {
                Color::White
            } else {
                Color::Black
            }
        })
        .unwrap_or_else(|| panel_contrast_fg(p))
}

fn mix_rgb(base: Rgb, target: Rgb, amount: f32) -> Rgb {
    fn channel(base: u8, target: u8, amount: f32) -> u8 {
        (f32::from(base) + (f32::from(target) - f32::from(base)) * amount).round() as u8
    }
    (
        channel(base.0, target.0, amount),
        channel(base.1, target.1, amount),
        channel(base.2, target.2, amount),
    )
}

fn relative_luminance(color: Rgb) -> f32 {
    fn channel(value: u8) -> f32 {
        let value = f32::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
}

fn color_to_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Reset => None,
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((255, 255, 255)),
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(_) => None,
    }
}
