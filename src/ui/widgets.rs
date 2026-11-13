//! Stateless drawing primitives shared by the original client UI.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::{text::display_width, Palette};

pub(crate) fn panel_contrast_fg(palette: &Palette) -> Color {
    match palette.panel_bg {
        Color::Reset => palette.surface_dim,
        color => color,
    }
}

pub(super) fn panel_inner(area: Rect) -> Option<Rect> {
    (area.width >= 2 && area.height >= 2)
        .then(|| Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2))
}

pub(crate) fn centered_popup(screen: Rect, width: u16, height: u16) -> Option<Rect> {
    let width = width.min(screen.width.saturating_sub(4));
    let height = height.min(screen.height.saturating_sub(2));
    if width < 4 || height < 4 {
        return None;
    }
    Some(Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    ))
}

/// Return the same clipped rectangles used for drawing and mouse hits. Advance
/// by requested widths, not visible widths: clipping must not promote a later
/// action into the position of a hidden earlier one. Fixed arrays need no heap.
pub(crate) fn button_row<const N: usize>(
    area: Rect,
    widths: [u16; N],
    gap: u16,
    offset: u16,
) -> [Rect; N] {
    if area.is_empty() {
        return [Rect::default(); N];
    }
    let total = widths
        .iter()
        .copied()
        .fold(0, u16::saturating_add)
        .saturating_add(gap.saturating_mul(N.saturating_sub(1).min(u16::MAX as usize) as u16));
    let mut x = area.x + area.width.saturating_sub(total) / 2;
    std::array::from_fn(|index| {
        let width = widths[index].min(area.right().saturating_sub(x));
        let rect = if width == 0 {
            Rect::default()
        } else {
            Rect::new(x, area.y + offset.min(area.height - 1), width, 1)
        };
        x = x.saturating_add(widths[index]).saturating_add(gap);
        rect
    })
}

pub(crate) fn paint_panel(
    buffer: &mut Buffer,
    area: Rect,
    color: Color,
    bg: Color,
) -> Option<Rect> {
    let area = area.intersection(buffer.area);
    let inner = panel_inner(area)?;
    let background = Style::default().bg(bg).remove_modifier(Modifier::DIM);
    let border = Style::default()
        .fg(color)
        .bg(bg)
        .remove_modifier(Modifier::DIM);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            buffer[(x, y)].set_symbol(" ").set_style(background);
        }
    }
    for x in area.x..area.right() {
        buffer[(x, area.y)]
            .set_symbol(if x == area.x {
                "┌"
            } else if x + 1 == area.right() {
                "┐"
            } else {
                "─"
            })
            .set_style(border);
        buffer[(x, area.bottom() - 1)]
            .set_symbol(if x == area.x {
                "└"
            } else if x + 1 == area.right() {
                "┘"
            } else {
                "─"
            })
            .set_style(border);
    }
    for y in area.y + 1..area.bottom() - 1 {
        buffer[(area.x, y)].set_symbol("│").set_style(border);
        buffer[(area.right() - 1, y)]
            .set_symbol("│")
            .set_style(border);
    }
    Some(inner)
}

pub(crate) fn paint_button(buffer: &mut Buffer, area: Rect, label: &str, style: Style) {
    let area = area.intersection(buffer.area);
    if area.is_empty() {
        return;
    }
    buffer.set_style(area, style);
    let width = display_width(label).min(usize::from(area.width)) as u16;
    paint_text_line(
        buffer,
        Rect::new(area.x + (area.width - width) / 2, area.y, width, 1),
        label,
        style,
    );
}

pub(crate) fn paint_text_line(buffer: &mut Buffer, area: Rect, text: &str, style: Style) {
    let area = area.intersection(buffer.area);
    if !area.is_empty() {
        buffer.set_stringn(area.x, area.y, text, usize::from(area.width), style);
    }
}
