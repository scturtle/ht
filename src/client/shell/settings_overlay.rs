use super::*;

fn draw_choice(
    buffer: &mut Buffer,
    rect: Rect,
    label: &str,
    selected: bool,
    current: bool,
    styles: &DialogStyles,
) {
    let style = if selected {
        styles.selected
    } else {
        styles.text
    };
    buffer.set_style(rect, style);
    let marker = if selected { "▸" } else { " " };
    let current = if current { " ✓" } else { "" };
    paint_text_line(buffer, rect, &format!(" {marker} {label}{current}"), style);
}

pub(super) fn render_settings_overlay(
    buffer: &mut Buffer,
    settings: &ClientSettingsOverlay,
    palette: &Palette,
) -> Option<OverlayRender> {
    let layout = DialogLayout::centered(buffer.area, (76, 22), [10, 12], None)?;
    let styles = DialogStyles::new(palette, palette.accent);
    layout.paint_panel(buffer, &styles);
    let body = layout.body;
    if body.width < 20 || body.height < 7 {
        return None;
    }

    paint_text_line(buffer, layout.body_row(0), " settings", styles.heading);

    let tabs = layout.body_row(1);
    let mut tab_x = tabs.x;
    let mut tab_hits = Vec::new();
    for section in &settings.sections {
        let label = format!(" {} ", section.label());
        let width = display_width(&label).min(tabs.right().saturating_sub(tab_x));
        let rect = Rect::new(tab_x, tabs.y, width, 1);
        let style = if *section == settings.section {
            styles.selected
        } else {
            styles.muted
        };
        buffer.set_style(rect, style);
        paint_text_line(buffer, rect, &label, style);
        tab_hits.push((rect, *section));
        tab_x = tab_x.saturating_add(width.saturating_add(1));
        if tab_x >= tabs.right() {
            break;
        }
    }
    paint_text_line(
        buffer,
        layout.body_row(2),
        &"─".repeat(body.width as usize),
        styles.text.fg(palette.surface0),
    );

    let content = Rect::new(
        body.x,
        body.y + 4,
        body.width,
        body.height.saturating_sub(6),
    );
    let mut choice_hits = Vec::new();
    match settings.section {
        ClientSettingsSection::Theme => {
            let visible = usize::from(content.height);
            let scroll = settings.selected.saturating_sub(visible.saturating_sub(1));
            for (visible_index, (index, name)) in crate::config::THEME_NAMES
                .iter()
                .enumerate()
                .skip(scroll)
                .take(visible)
                .enumerate()
            {
                let rect = Rect::new(
                    content.x,
                    content.y + visible_index as u16,
                    content.width,
                    1,
                );
                draw_choice(
                    buffer,
                    rect,
                    name,
                    index == settings.selected,
                    super::super::settings::normalized_theme_name(name)
                        == super::super::settings::normalized_theme_name(
                            &settings.original_theme_name,
                        ),
                    &styles,
                );
                choice_hits.push((rect, index));
            }
        }
        ClientSettingsSection::Indicators => {
            render_choice_section(
                buffer,
                content,
                "agent status indicators",
                "choose color dots or distinct symbols for each state",
                &["color dots  ● ● ● ○ ·", "distinct symbols  × ◐ ✓ ○ ·"],
                settings.selected,
                &styles,
                &mut choice_hits,
            );
        }
        ClientSettingsSection::Sound => {
            render_choice_section(
                buffer,
                content,
                "sound alerts",
                "play sounds when agents change state in background",
                &["on", "off"],
                settings.selected,
                &styles,
                &mut choice_hits,
            );
        }
        ClientSettingsSection::Toast => {
            render_choice_section(
                buffer,
                content,
                "notification popups",
                "choose where background popup notifications should appear",
                &["off", "inside herdr", "via terminal", "via system"],
                settings.selected,
                &styles,
                &mut choice_hits,
            );
        }
    }

    layout.paint_buttons(buffer, &styles, [" ↵ apply ", " esc close "]);
    paint_text_line(
        buffer,
        layout.body_row(body.height - 1),
        " ↑↓ select  tab section",
        styles.muted,
    );
    let [primary, cancel] = layout.buttons;
    Some(OverlayRender {
        area: layout.area,
        primary,
        cancel,
        settings_popup: layout.area,
        settings_tabs: tab_hits,
        settings_choices: choice_hits,
        ..OverlayRender::default()
    })
}

fn render_choice_section(
    buffer: &mut Buffer,
    area: Rect,
    title: &str,
    description: &str,
    choices: &[&str],
    selected: usize,
    styles: &DialogStyles,
    hits: &mut Vec<(Rect, usize)>,
) {
    paint_text_line(
        buffer,
        Rect::new(area.x, area.y, area.width, 1),
        title,
        styles.heading,
    );
    paint_text_line(
        buffer,
        Rect::new(area.x, area.y + 1, area.width, 1),
        description,
        styles.muted,
    );
    let row_gap = u16::from(choices.len() > 2);
    for (index, choice) in choices.iter().enumerate() {
        let y = area.y + 3 + index as u16 * (1 + row_gap);
        if y >= area.bottom() {
            break;
        }
        let rect = Rect::new(area.x, y, area.width, 1);
        draw_choice(buffer, rect, choice, index == selected, false, styles);
        hits.push((rect, index));
    }
}
