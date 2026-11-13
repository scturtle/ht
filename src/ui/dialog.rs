//! Small modal chrome: pure geometry, semantic styles and stateless painting.
//! Business actions, text editing and confirmation state remain in client/shell.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::{
    widgets::{
        button_row, centered_popup, paint_button, paint_panel, panel_contrast_fg, panel_inner,
    },
    Palette,
};

pub(crate) struct DialogLayout<const N: usize> {
    pub(crate) area: Rect,
    pub(crate) buttons: [Rect; N],
    pub(crate) body: Rect,
    /// Reserved full-width row, for action buttons or an informational hint.
    pub(crate) footer: Rect,
}

impl<const N: usize> DialogLayout<N> {
    /// `body_rows` caps compact dialogs; `None` fills the body and puts actions
    /// at the bottom. Both modes reserve the footer before laying out text.
    pub(crate) fn centered(
        screen: Rect,
        size: (u16, u16),
        widths: [u16; N],
        body_rows: Option<u16>,
    ) -> Option<Self> {
        let area = centered_popup(screen, size.0, size.1)?;
        let inner = panel_inner(area)?;
        // Reserve the footer from body contents. On short screens omit
        // unavailable body rows, never overwrite a button
        // or border with a text editor and its cursor.
        let body = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner
                .height
                .saturating_sub(1)
                .min(body_rows.unwrap_or(u16::MAX)),
        );
        let footer = Rect::new(inner.x, inner.y + body.height, inner.width, 1);
        let buttons = button_row(footer, widths, 2, 0);
        Some(Self {
            area,
            body,
            footer,
            buttons,
        })
    }

    pub(crate) fn body_row(&self, offset: u16) -> Rect {
        if offset >= self.body.height {
            return Rect::default();
        }
        Rect::new(self.body.x, self.body.y + offset, self.body.width, 1)
    }

    pub(crate) fn paint_panel(&self, buffer: &mut Buffer, styles: &DialogStyles) {
        let _ = paint_panel(buffer, self.area, styles.border, styles.background);
    }

    /// First button is primary; the remaining buttons use the secondary style.
    pub(crate) fn paint_buttons(
        &self,
        buffer: &mut Buffer,
        styles: &DialogStyles,
        labels: [&str; N],
    ) {
        let primary = styles.selected;
        let secondary = styles.field.add_modifier(Modifier::BOLD);
        for (index, (area, label)) in self.buttons.iter().zip(labels).enumerate() {
            paint_button(
                buffer,
                *area,
                label,
                if index == 0 { primary } else { secondary },
            );
        }
    }
}

pub(crate) struct DialogStyles {
    pub(crate) heading: Style,
    pub(crate) text: Style,
    pub(crate) field: Style,
    pub(crate) selected: Style,
    pub(crate) muted: Style,
    pub(crate) subtle: Style,
    border: Color,
    background: Color,
}

impl DialogStyles {
    pub(crate) fn new(palette: &Palette, accent: Color) -> Self {
        let text = Style::default().fg(palette.text).bg(palette.panel_bg);
        let field = Style::default().fg(palette.text).bg(palette.surface0);
        Self {
            heading: text.add_modifier(Modifier::BOLD),
            text,
            field,
            border: accent,
            background: palette.panel_bg,
            selected: Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(accent)
                .add_modifier(Modifier::BOLD),
            muted: Style::default().fg(palette.overlay1).bg(palette.panel_bg),
            subtle: Style::default().fg(palette.overlay0).bg(palette.panel_bg),
        }
    }
}

#[cfg(test)]
mod tests;
