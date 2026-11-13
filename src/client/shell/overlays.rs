use super::*;
use crate::ui::{
    dialog::{DialogLayout, DialogStyles},
    widgets::{paint_button, paint_panel, paint_text_line},
};

mod settings_overlay;

#[derive(Default)]
pub(crate) struct OverlayRender {
    pub(crate) area: Rect,
    pub(crate) menu_rows: Vec<(Rect, usize)>,
    pub(crate) primary: Rect,
    pub(crate) clear: Rect,
    pub(crate) cancel: Rect,
    pub(crate) navigator_popup: Rect,
    pub(crate) navigator_rows: Vec<(Rect, ClientNavigatorTarget)>,
    pub(crate) navigator_scrollbar: Rect,
    pub(crate) navigator_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) help_popup: Rect,
    pub(crate) help_scrollbar: Rect,
    pub(crate) help_scroll_metrics: Option<crate::pane::ScrollMetrics>,
    pub(crate) help_max_scroll: usize,
    pub(crate) settings_popup: Rect,
    pub(crate) settings_tabs: Vec<(Rect, ClientSettingsSection)>,
    pub(crate) settings_choices: Vec<(Rect, usize)>,
    pub(crate) cursor: Option<crate::protocol::CursorState>,
}

pub(crate) fn render_client_overlay(
    b: &mut Buffer,
    o: &ClientShellOverlay,
    endpoints: &ClientShellEndpoint,
    k: &LiveKeybindConfig,
    p: &Palette,
) -> Option<OverlayRender> {
    if !matches!(
        o,
        ClientShellOverlay::Navigator(_)
            | ClientShellOverlay::ContextMenu(_)
            | ClientShellOverlay::GlobalMenu(_)
    ) {
        for y in b.area.y..b.area.bottom() {
            for x in b.area.x..b.area.right() {
                let c = &mut b[(x, y)];
                c.set_style(c.style().add_modifier(Modifier::DIM));
            }
        }
    }
    match o {
        ClientShellOverlay::Rename(v) => render_rename_overlay(b, v, p),
        ClientShellOverlay::ConfirmClose(v) => render_confirm_close_overlay(b, v, p),
        ClientShellOverlay::Help(v) => render_help_overlay(b, v, k, p),
        ClientShellOverlay::Navigator(v) => render_navigator_overlay(b, v, endpoints, p),
        ClientShellOverlay::Settings(v) => settings_overlay::render_settings_overlay(b, v, p),
        ClientShellOverlay::ContextMenu(_) | ClientShellOverlay::GlobalMenu(_) => None,
    }
}

pub(crate) fn render_global_menu(
    buffer: &mut Buffer,
    launcher: Rect,
    menu: &ClientGlobalMenuOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let items = super::super::global_menu::global_menu_items();
    let screen = buffer.area;
    let width = items
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(8)
        .saturating_add(4)
        .min(screen.width.max(1));
    let height = (items.len() as u16)
        .saturating_add(2)
        .min(screen.height.max(1));
    let x = launcher
        .right()
        .saturating_sub(width)
        .min(screen.right().saturating_sub(width));
    let y = launcher.y.saturating_sub(height).max(screen.y);
    let rect = Rect::new(x, y, width, height);
    let inner = paint_panel(buffer, rect, palette.accent, palette.panel_bg)?;
    let labels = items.iter().map(|(label, _)| *label);
    let rows = paint_menu_rows(buffer, inner, labels, menu.highlighted, 1, palette);
    Some(OverlayRender {
        area: rect,
        menu_rows: rows,
        ..OverlayRender::default()
    })
}

pub(crate) fn render_context_menu(
    buffer: &mut Buffer,
    menu: &ClientContextMenuOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let items = menu.items();
    let screen = buffer.area;
    let max_item_width = items
        .iter()
        .map(|item| display_width(item.label))
        .max()
        .unwrap_or(0);
    let width = max_item_width
        .saturating_add(4)
        .max(14)
        .min(screen.width.max(1));
    let height = (items.len() as u16)
        .saturating_add(2)
        .min(screen.height.max(1));
    let x = menu
        .x
        .min(screen.x.saturating_add(screen.width.saturating_sub(width)));
    let y = menu.y.min(
        screen
            .y
            .saturating_add(screen.height.saturating_sub(height)),
    );
    let rect = Rect::new(x, y, width, height);
    let inner = paint_panel(buffer, rect, palette.accent, palette.panel_bg)?;
    let labels = items.iter().map(|item| item.label);
    let rows = paint_menu_rows(buffer, inner, labels, menu.highlighted, 0, palette);
    Some(OverlayRender {
        area: rect,
        menu_rows: rows,
        ..OverlayRender::default()
    })
}

/// Paint the rows of an already anchored menu panel and return the same
/// rectangles as hits. Anchoring, minimum width and the text inset stay with
/// each menu; row clipping, selection styles, text and hits are shared.
fn paint_menu_rows<'a>(
    buffer: &mut Buffer,
    inner: Rect,
    labels: impl Iterator<Item = &'a str>,
    highlighted: usize,
    text_inset: u16,
    palette: &Palette,
) -> Vec<(Rect, usize)> {
    let selected = Style::default()
        .fg(panel_contrast_fg(palette))
        .bg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(palette.text).bg(palette.panel_bg);
    let mut rows = Vec::new();
    for (index, label) in labels.enumerate() {
        let row_y = inner.y.saturating_add(index as u16);
        if row_y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);
        let style = if index == highlighted {
            selected
        } else {
            normal
        };
        buffer.set_style(row, style);
        let text_x = row.x.saturating_add(text_inset);
        put_text(
            buffer,
            text_x,
            row.y,
            row.width.saturating_sub(text_inset),
            label,
            style,
        );
        rows.push((row, index));
    }
    rows
}

fn render_rename_overlay(
    buffer: &mut Buffer,
    rename: &ClientRenameOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let layout = DialogLayout::centered(buffer.area, (56, 7), [8, 10, 12], Some(3))?;
    let styles = DialogStyles::new(palette, palette.accent);
    layout.paint_panel(buffer, &styles);
    layout.paint_buttons(buffer, &styles, [" ↵ save ", " ^c clear ", " esc cancel "]);
    paint_text_line(buffer, layout.body_row(0), rename.title, styles.heading);
    let input = layout.body_row(2);
    buffer.set_style(input, styles.field);
    let cursor = text_editor::render(
        buffer,
        Rect::new(
            input.x.saturating_add(1),
            input.y,
            input.width.saturating_sub(1),
            input.height,
        ),
        &rename.input,
        styles.field,
    );
    let [primary, clear, cancel] = layout.buttons;
    Some(OverlayRender {
        area: layout.area,
        primary,
        clear,
        cancel,
        cursor,
        ..OverlayRender::default()
    })
}

fn render_navigator_overlay(
    b: &mut Buffer,
    n: &ClientNavigatorOverlay,
    endpoints: &ClientShellEndpoint,
    p: &Palette,
) -> Option<OverlayRender> {
    let a = b.area;
    let width = a.width.saturating_sub(4).min(116);
    let height = a.height.saturating_sub(2).min(42);
    if width < 4 || height < 7 {
        return None;
    }
    let q = Rect::new(
        a.x + (a.width - width) / 2,
        a.y + (a.height - height) / 2,
        width,
        height,
    )
    .intersection(a);
    let i = paint_panel(b, q, p.accent, p.panel_bg)?;
    let rows = super::navigator::navigator_rows(endpoints);
    let terminal_count = rows
        .iter()
        .filter(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. }))
        .count();
    let count = format!(
        "{terminal_count} {}",
        if terminal_count == 1 {
            "terminal"
        } else {
            "terminals"
        }
    );
    put_text(
        b,
        q.x + 2,
        q.y,
        q.width.saturating_sub(4),
        &format!(" Go to · {count} "),
        Style::default().fg(p.accent).bg(p.panel_bg),
    );
    let body = Rect::new(i.x, i.y, i.width, i.height.saturating_sub(3));
    let selected = super::navigator::navigator_selected_index(&rows, n).unwrap_or(0);
    let max = rows.len().saturating_sub(body.height as usize);
    let scroll = n
        .scroll
        .max(selected.saturating_sub(body.height.saturating_sub(1) as usize))
        .min(selected)
        .min(max);
    let metrics = crate::pane::ScrollMetrics {
        offset_from_bottom: max.saturating_sub(scroll),
        max_offset_from_bottom: max,
        viewport_rows: usize::from(body.height),
    };
    let scrollbar =
        (max > 0 && body.width > 1).then_some(Rect::new(body.right() - 1, body.y, 1, body.height));
    let row_width = body.width.saturating_sub(u16::from(scrollbar.is_some()));
    let mut row_hits = Vec::new();
    if rows.is_empty() {
        put_text(
            b,
            body.x,
            body.y,
            body.width,
            " No agents or terminals",
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
    }
    for (ix, r) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(body.height as usize)
    {
        let rect = Rect::new(body.x, body.y + (ix - scroll) as u16, row_width, 1);
        row_hits.push((rect, r.target.clone()));
        let st = if r.stale {
            Style::default()
                .fg(p.overlay0)
                .bg(if ix == selected {
                    p.surface0
                } else {
                    p.panel_bg
                })
                .add_modifier(Modifier::DIM)
        } else if ix == selected {
            Style::default()
                .fg(panel_contrast_fg(p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.text).bg(p.panel_bg)
        };
        let is_pane = matches!(r.target, ClientNavigatorTarget::Pane { .. });
        let connector = if !is_pane {
            ""
        } else if rows
            .get(ix + 1)
            .is_some_and(|next| matches!(next.target, ClientNavigatorTarget::Pane { .. }))
        {
            "├─ "
        } else {
            "└─ "
        };
        let padding = u16::from(r.depth.saturating_sub(u8::from(is_pane))) * 2 + 1;
        let connector_x = rect.x + padding;
        let indent = format!("{:width$}{connector}", "", width = usize::from(padding));
        let current = if r.current { "◆ " } else { "" };
        let status = r.status.map(status_dot).unwrap_or_default();
        let status_separator = if status.is_empty() { "" } else { " " };
        let label = format!("{indent}{current}{status}{status_separator}{}", r.label);
        let st = if r.status.is_none() {
            st.add_modifier(Modifier::BOLD)
        } else {
            st
        };
        b.set_style(rect, st);
        let columns = if r.status.is_some() {
            if rect.width >= 64 {
                24
            } else if rect.width >= 36 {
                12
            } else {
                0
            }
        } else {
            0
        };
        put_text(
            b,
            rect.x,
            rect.y,
            rect.width.saturating_sub(columns),
            &label,
            st,
        );
        if is_pane {
            put_text(
                b,
                connector_x,
                rect.y,
                rect.right().saturating_sub(connector_x).min(2),
                connector,
                if r.stale || ix == selected {
                    st
                } else {
                    st.fg(p.overlay0)
                },
            );
        }
        if let Some(status) = r.status {
            let prefix = format!("{indent}{current}");
            let status_style = if r.stale || ix == selected {
                st
            } else {
                Style::default().fg(status_color(status, p)).bg(p.panel_bg)
            };
            put_text(
                b,
                rect.x.saturating_add(display_width(&prefix)),
                rect.y,
                display_width(status_dot(status)),
                status_dot(status),
                status_style,
            );
            let meta_style = if r.stale || ix == selected {
                st
            } else {
                st.fg(p.overlay0)
            };
            if columns > 0 {
                put_text(
                    b,
                    rect.right() - columns + 1,
                    rect.y,
                    11,
                    r.agent.as_deref().unwrap_or("terminal"),
                    meta_style,
                );
            }
            if columns == 24 {
                put_text(
                    b,
                    rect.right() - 11,
                    rect.y,
                    11,
                    if r.agent.is_some() {
                        status_text(status)
                    } else {
                        "shell"
                    },
                    meta_style,
                );
            }
        }
        if r.status.is_none() && !r.meta.is_empty() {
            let label_width = display_width(&label).min(rect.width);
            let meta = Rect::new(
                rect.x.saturating_add(label_width).saturating_add(1),
                rect.y,
                rect.width.saturating_sub(label_width.saturating_add(1)),
                1,
            );
            put_right_text(b, meta, rect.y, &r.meta, st)
        }
    }
    if let Some(track) = scrollbar {
        crate::ui::render_scrollbar_buffer(b, metrics, track, p.overlay0, p.overlay1, "▐");
    }
    if let Some(r) = rows.get(selected) {
        put_text(
            b,
            i.x,
            i.bottom() - 3,
            i.width,
            &format!(" {}", r.detail),
            Style::default().fg(p.subtext0).bg(p.panel_bg),
        );
        put_text(
            b,
            i.x,
            i.bottom() - 2,
            i.width,
            &format!(" {}", r.meta),
            Style::default().fg(p.overlay0).bg(p.panel_bg),
        );
    }
    put_text(
        b,
        i.x,
        i.bottom() - 1,
        i.width,
        " ↑↓/j/k rows · ←→ workspace · enter open · esc close",
        Style::default().fg(p.overlay0).bg(p.panel_bg),
    );
    Some(OverlayRender {
        area: q,
        primary: Rect::default(),
        clear: Rect::default(),
        cancel: Rect::default(),
        navigator_popup: q,
        navigator_rows: row_hits,
        navigator_scrollbar: scrollbar.unwrap_or_default(),
        navigator_scroll_metrics: Some(metrics),
        ..OverlayRender::default()
    })
}

fn help_lines(
    keybinds: &LiveKeybindConfig,
    query: &str,
    palette: &Palette,
    styles: &DialogStyles,
) -> Vec<(usize, ratatui::text::Line<'static>)> {
    use ratatui::text::{Line, Span};

    let groups = crate::input::filter_keybind_help_groups(
        crate::input::keybind_help_groups(&keybinds.keybinds, &keybinds.prefix),
        query,
    );
    let key_width = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|(key, _)| key.chars().count()))
        .max()
        .unwrap_or(8);
    if groups.is_empty() {
        let message = " no matching keybinds";
        return vec![(
            message.chars().count(),
            Line::from(Span::styled(message, styles.muted)),
        )];
    }

    let group_style = styles.heading.fg(palette.accent);
    let key_style = styles.heading.fg(palette.mauve);
    let mut lines = Vec::new();
    for (group, entries) in groups {
        lines.push((
            group.len() + 1,
            Line::from(Span::styled(format!(" {group}"), group_style)),
        ));
        for (key, label) in entries {
            let padded_key = format!(" {key:<key_width$} ");
            let width = padded_key.chars().count() + label.chars().count();
            lines.push((
                width,
                Line::from(vec![
                    Span::styled(padded_key, key_style),
                    Span::styled(label.into_owned(), styles.text),
                ]),
            ));
        }
        lines.push((0, Line::raw("")));
    }
    lines
}

fn render_help_overlay(
    b: &mut Buffer,
    h: &ClientHelpOverlay,
    k: &LiveKeybindConfig,
    p: &Palette,
) -> Option<OverlayRender> {
    use ratatui::widgets::{Paragraph, Widget, Wrap};

    // No footer actions: the reserved row carries the keyboard hint instead.
    let layout = DialogLayout::centered(b.area, (76, 22), [], None)?;
    let styles = DialogStyles::new(p, p.accent);
    layout.paint_panel(b, &styles);
    let i = layout.body;
    if i.width < 20 || i.height < 5 {
        return None;
    }
    paint_text_line(b, layout.body_row(0), "keybinds", styles.heading);
    let close = Rect::new(i.right() - 13, i.y, 13, 1);
    paint_button(
        b,
        close,
        if h.search_focused {
            " esc back "
        } else {
            " esc close "
        },
        styles.selected,
    );
    let sy = i.y + 1;
    paint_text_line(
        b,
        layout.body_row(1),
        if h.search_focused {
            " / "
        } else {
            " / press / to filter by command or shortcut"
        },
        if h.search_focused {
            styles.text
        } else {
            styles.subtle
        },
    );
    let cursor = if h.search_focused {
        text_editor::render(
            b,
            Rect::new(i.x + 3, sy, i.width.saturating_sub(3), 1),
            &h.query,
            styles.text,
        )
    } else {
        None
    };

    let body = Rect::new(i.x, i.y + 3, i.width, i.height.saturating_sub(4));
    let lines = help_lines(k, &h.query, p, &styles);
    let viewport_rows = usize::from(body.height.max(1));
    let wrapped_rows = |width: u16| {
        let width = usize::from(width.max(1));
        lines
            .iter()
            .map(|(line_width, _)| line_width.max(&1).div_ceil(width))
            .sum::<usize>()
    };
    let needs_scrollbar = wrapped_rows(body.width) > viewport_rows;
    let text_area = if needs_scrollbar {
        Rect::new(body.x, body.y, body.width.saturating_sub(1), body.height)
    } else {
        body
    };
    let total_rows = wrapped_rows(text_area.width);
    let max_scroll = total_rows.saturating_sub(viewport_rows);
    let scroll = h.scroll.min(max_scroll);
    let metrics = crate::pane::ScrollMetrics {
        offset_from_bottom: max_scroll.saturating_sub(scroll),
        max_offset_from_bottom: max_scroll,
        viewport_rows,
    };
    let scrollbar = needs_scrollbar.then_some(Rect::new(
        body.right().saturating_sub(1),
        body.y,
        1,
        body.height,
    ));
    Widget::render(
        Paragraph::new(lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>())
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        text_area,
        b,
    );
    if let Some(track) = scrollbar {
        if let Some(thumb) = crate::ui::scrollbar_thumb(metrics, track) {
            for y in track.y..track.bottom() {
                b[(track.x, y)].set_symbol("▐").set_style(styles.subtle);
            }
            for y in thumb.top..thumb.top.saturating_add(thumb.len) {
                b[(track.x, y)].set_symbol("▐").set_style(styles.muted);
            }
        }
    }

    paint_text_line(
        b,
        layout.footer,
        if h.search_focused {
            " edit ←→/home/end · kill ^u/^k · yank ^y · scroll ↑↓ · back esc"
        } else {
            " search / · scroll j/k/↑↓/pgup/pgdn · close esc/enter"
        },
        styles.subtle,
    );
    Some(OverlayRender {
        area: layout.area,
        cancel: close,
        help_popup: layout.area,
        help_scrollbar: scrollbar.unwrap_or_default(),
        help_scroll_metrics: Some(metrics),
        help_max_scroll: max_scroll,
        cursor,
        ..OverlayRender::default()
    })
}
fn render_confirm_close_overlay(
    buffer: &mut Buffer,
    close: &ClientConfirmCloseOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let layout = DialogLayout::centered(buffer.area, (64, 6), [13, 12], Some(3))?;
    let styles = DialogStyles::new(palette, palette.red);
    layout.paint_panel(buffer, &styles);
    layout.paint_buttons(buffer, &styles, [" ↵ confirm ", " esc cancel "]);
    paint_text_line(
        buffer,
        layout.body_row(0),
        &format!(" {}", close.title),
        styles.heading.fg(palette.red),
    );
    paint_text_line(
        buffer,
        layout.body_row(1),
        &format!(" {}", close.detail),
        styles.text,
    );
    let [primary, cancel] = layout.buttons;
    Some(OverlayRender {
        area: layout.area,
        primary,
        cancel,
        ..OverlayRender::default()
    })
}
