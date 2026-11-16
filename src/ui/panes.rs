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

/// A zoomed pane fills the surface; all split dividers are hidden.
pub(crate) fn zoomed_pane_info(id: crate::layout::PaneId, area: Rect) -> PaneInfo {
    PaneInfo {
        id,
        rect: area,
        inner_rect: area,
        borders: Borders::NONE,
        is_focused: true,
    }
}

/// Shared inner dividers only: no outer frame, gaps, or scrollbar columns.
pub(crate) fn apply_pane_chrome(panes: Vec<PaneInfo>) -> Vec<PaneInfo> {
    let outer_left = panes.iter().map(|info| info.rect.x).min().unwrap_or(0);
    let outer_top = panes.iter().map(|info| info.rect.y).min().unwrap_or(0);
    panes
        .into_iter()
        .map(|mut info| {
            let mut borders = Borders::NONE;
            // Each divider is owned by the pane to its right or below.
            if info.rect.x > outer_left {
                borders.insert(Borders::LEFT);
            }
            if info.rect.y > outer_top {
                borders.insert(Borders::TOP);
            }
            info.borders = borders;
            info.inner_rect = pane_inner_rect(info.rect, borders);
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
    host_theme: crate::terminal::TerminalTheme,
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

fn automatic_selection_style(p: &Palette, host_theme: crate::terminal::TerminalTheme) -> Style {
    let bg = automatic_selection_bg(p, host_theme);
    Style::reset().fg(selection_fg_for_bg(bg, p)).bg(bg)
}

fn automatic_selection_bg(p: &Palette, host_theme: crate::terminal::TerminalTheme) -> Color {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TileLayout;
    use ratatui::layout::Direction;

    #[test]
    fn single_and_zoomed_panes_fill_the_area_without_a_frame() {
        let (layout, id) = TileLayout::new();
        for area in [Rect::new(0, 0, 80, 24), Rect::new(3, 5, 1, 1)] {
            let panes = apply_pane_chrome(layout.panes(area));
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].rect, area);
            assert_eq!(panes[0].inner_rect, area);
            assert_eq!(panes[0].borders, Borders::NONE);
            let zoomed = zoomed_pane_info(id, area);
            assert_eq!(zoomed.inner_rect, area);
            assert_eq!(zoomed.borders, Borders::NONE);
        }
        assert!(apply_pane_chrome(Vec::new()).is_empty());
    }

    #[test]
    fn nested_splits_have_one_inner_divider_and_no_gaps_or_outer_frame() {
        for first_direction in [Direction::Horizontal, Direction::Vertical] {
            let (mut layout, _) = TileLayout::new();
            layout.split_focused(first_direction);
            layout.split_focused(match first_direction {
                Direction::Horizontal => Direction::Vertical,
                Direction::Vertical => Direction::Horizontal,
            });
            let area = Rect::new(3, 5, 80, 24);
            let original = layout.panes(area);
            let panes = apply_pane_chrome(original.clone());
            for (pane, before) in panes.iter().zip(&original) {
                assert_eq!(pane.rect, before.rect, "chrome must not introduce gaps");
                assert_eq!(pane.id, before.id);
                assert_eq!(pane.is_focused, before.is_focused);
                let left = u16::from(pane.rect.x > area.x);
                let top = u16::from(pane.rect.y > area.y);
                assert_eq!(pane.borders.contains(Borders::LEFT), left != 0);
                assert_eq!(pane.borders.contains(Borders::TOP), top != 0);
                assert!(!pane.borders.intersects(Borders::RIGHT | Borders::BOTTOM));
                assert_eq!(
                    pane.inner_rect,
                    Rect::new(
                        pane.rect.x + left,
                        pane.rect.y + top,
                        pane.rect.width - left,
                        pane.rect.height - top,
                    )
                );
            }
        }
    }
}
