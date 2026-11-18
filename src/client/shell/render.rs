use super::*;

#[path = "../shell/overlays.rs"]
mod overlays;
#[path = "../shell/tabs.rs"]
mod tabs;

pub(super) use overlays::{render_client_overlay, render_context_menu, render_global_menu};
pub(super) use tabs::{render_tab_bar, TabBarEntries};

const COPY_SEARCH_PREFIX_WIDTH: u16 = 8;
const COPY_SEARCH_MIN_FIELD_WIDTH: u16 = 8;
const COPY_SEARCH_MIN_WIDTH: u16 = COPY_SEARCH_PREFIX_WIDTH + COPY_SEARCH_MIN_FIELD_WIDTH;

struct ModeStyles {
    base: Style,
    key: Style,
    badge: Style,
    input: Style,
    input_cursor: Style,
}

impl ModeStyles {
    fn new(palette: &Palette) -> Self {
        Self {
            base: Style::default().fg(palette.overlay0).bg(palette.panel_bg),
            key: Style::default()
                .fg(palette.accent)
                .bg(palette.panel_bg)
                .add_modifier(Modifier::BOLD),
            badge: Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
            input: Style::default().fg(palette.text).bg(palette.panel_bg),
            input_cursor: Style::default().fg(palette.panel_bg).bg(palette.text),
        }
    }
}

fn prefix_rhs(bindings: &crate::config::ActionKeybinds) -> String {
    bindings
        .prefix_rhs_label()
        .unwrap_or_else(|| "unset".to_owned())
}

fn put_mode_segments<'a>(
    buffer: &mut Buffer,
    area: Rect,
    segments: impl IntoIterator<Item = (&'a str, Style)>,
) {
    let mut x = area.x;
    for (text, style) in segments {
        if x >= area.right() {
            break;
        }
        x = put_segment(buffer, x, area.y, area.right(), text, style);
    }
}

pub(super) enum ModeHint<'a> {
    Prefix,
    Copy(&'a ClientCopyModeState),
}

impl ModeHint<'_> {
    pub(super) fn fits_inline(&self, area: Rect) -> bool {
        // Passive hints can clip or disappear; the search prompt needs room to type.
        !matches!(self, Self::Copy(copy) if copy.search_prompt.is_some() && area.width < COPY_SEARCH_MIN_WIDTH)
    }

    fn render(
        self,
        buffer: &mut Buffer,
        area: Rect,
        keybinds: &LiveKeybindConfig,
        styles: ModeStyles,
    ) {
        let ModeStyles {
            base, key, badge, ..
        } = styles;
        match self {
            Self::Prefix => put_mode_segments(
                buffer,
                area,
                [
                    (" PREFIX ", badge),
                    (" ", base),
                    ("esc", key),
                    (" cancel  ", base),
                    (&keybinds.primary_prefix_label(), key),
                    (" send prefix  ", base),
                    (&prefix_rhs(&keybinds.keybinds.workspace_picker), key),
                    (" workspace nav  ", base),
                    (&prefix_rhs(&keybinds.keybinds.help), key),
                    (" keybinds", base),
                ],
            ),
            Self::Copy(copy_mode) => {
                if let Some(prompt) = &copy_mode.search_prompt {
                    render_copy_search(buffer, area, prompt, styles);
                    return;
                }
                let select = if copy_mode.selection.is_some() {
                    " selecting  "
                } else {
                    " select  "
                };
                let match_status = copy_mode.search_current_global.map_or_else(
                    || {
                        if copy_mode.search_query.is_empty() {
                            String::new()
                        } else {
                            " 0/0".into()
                        }
                    },
                    |current| format!(" {}/{}", current + 1, copy_mode.search_total),
                );
                let (exit_keys, exit_label) =
                    if copy_mode.search_query.is_empty() && copy_mode.selection.is_none() {
                        ("q/esc", " exit  ")
                    } else {
                        ("esc", " clear  q exit  ")
                    };
                put_mode_segments(
                    buffer,
                    area,
                    [
                        (" COPY ", badge),
                        (" ", base),
                        ("J/K ^e/^y", key),
                        (" scroll  ", base),
                        ("H/M/L", key),
                        (" jump  ", base),
                        ("z", key),
                        (" center  ", base),
                        ("/? n/N", key),
                        (&format!(" search{match_status}  "), base),
                        ("v/V", key),
                        (select, base),
                        ("y/↵", key),
                        (" copy  ", base),
                        (exit_keys, key),
                        (exit_label, base),
                        ("h/j/k/l w/b/e { }", key),
                        (" move", base),
                    ],
                );
            }
        }
    }
}

fn render_copy_search(
    buffer: &mut Buffer,
    area: Rect,
    prompt: &ClientCopySearchPrompt,
    styles: ModeStyles,
) {
    let marker = match prompt.direction {
        crate::actions::PaneCopySearchDirection::Forward => "/",
        crate::actions::PaneCopySearchDirection::Backward => "?",
    };
    put_mode_segments(
        buffer,
        area,
        [
            (" COPY ", styles.badge),
            (" ", styles.base),
            (marker, styles.key),
        ],
    );
    let prefix = COPY_SEARCH_PREFIX_WIDTH.min(area.width);
    let footer = "  enter search  esc cancel";
    let footer_width = display_width(footer);
    let footer_width = if area.width >= COPY_SEARCH_MIN_WIDTH + footer_width {
        footer_width
    } else {
        0
    };
    let field = Rect::new(
        area.x + prefix,
        area.y,
        area.width - prefix - footer_width,
        1,
    );
    if let Some(cursor) = text_editor::render(buffer, field, &prompt.query, styles.input) {
        buffer[(cursor.x, cursor.y)].set_style(styles.input_cursor);
    }
    put_text(
        buffer,
        area.right() - footer_width,
        area.y,
        footer_width,
        footer,
        styles.base,
    );
}

pub(super) fn tab_bar_hint_area(area: Rect, new_tab: Rect) -> Rect {
    // Use only the space left by the normal tab layout, including its overflow controls.
    if new_tab.is_empty() {
        Rect::default()
    } else {
        let x = new_tab.right();
        Rect::new(x, area.y, area.right().saturating_sub(x), 1)
    }
}

pub(super) fn render_tab_bar_hint(
    buffer: &mut Buffer,
    area: Rect,
    hint: ModeHint<'_>,
    config: &ClientShellConfig,
) {
    if !area.is_empty() {
        hint.render(
            buffer,
            area,
            &config.keybinds,
            ModeStyles::new(&config.palette),
        );
    }
}

pub(super) fn render_mode_bar(
    buffer: &mut Buffer,
    pane_area: Rect,
    mode: ClientShellMode,
    copy_mode: Option<&ClientCopyModeState>,
    endpoint_error: Option<&str>,
    keybinds: &LiveKeybindConfig,
    palette: &Palette,
) -> Option<Rect> {
    if (mode == ClientShellMode::Terminal && endpoint_error.is_none()) || pane_area.is_empty() {
        return None;
    }

    let bar = Rect::new(
        pane_area.x,
        pane_area.y + pane_area.height.saturating_sub(1),
        pane_area.width,
        1,
    );
    let styles = ModeStyles::new(palette);
    let ModeStyles {
        base, key, badge, ..
    } = styles;
    for x in bar.x..bar.x + bar.width {
        buffer[(x, bar.y)].set_symbol(" ").set_style(base);
    }

    if let Some(error) = endpoint_error {
        put_mode_segments(
            buffer,
            bar,
            [(" ERROR ", badge), (&format!(" {error}"), base)],
        );
    } else {
        match mode {
            ClientShellMode::Prefix => ModeHint::Prefix.render(buffer, bar, keybinds, styles),
            ClientShellMode::Navigate => put_mode_segments(
                buffer,
                bar,
                [
                    (" NAVIGATE ", badge),
                    (" esc back  ", base),
                    ("↑/↓", key),
                    (" workspace  ", base),
                    ("tab", key),
                    (" pane  ", base),
                    (&prefix_rhs(&keybinds.keybinds.help), key),
                    (" keybinds", base),
                ],
            ),
            ClientShellMode::Copy => {
                ModeHint::Copy(copy_mode?).render(buffer, bar, keybinds, styles)
            }
            ClientShellMode::Terminal => unreachable!(),
        }
    }
    Some(bar)
}

pub(super) struct ShellRenderState<'a> {
    pub(super) tab_scroll: &'a mut usize,
    pub(super) reveal_focused_tab: &'a mut bool,
    pub(super) tab_drag_insert_index: Option<usize>,
}

pub(super) fn render_shell(
    buffer: &mut Buffer,
    layout: ClientShellLayout,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: ShellRenderState<'_>,
) -> ShellHitMap {
    let mut hits = ShellHitMap::default();
    let bar = TabBarEntries::new(snapshot);
    let mobile_pane_bar = layout.mobile_header.height > 1;
    if layout.mobile_header.height > 0 {
        super::mobile::render_mobile_header(
            buffer,
            layout.mobile_header,
            snapshot,
            config,
            &mut hits,
        );
    }
    let mut tab_area = layout.tab_bar;
    if mobile_pane_bar {
        // Keep the switch button beside the same tab and pane targets as
        // desktop, without shrinking the PTY.
        tab_area = Rect::new(
            layout.mobile_header.x,
            layout.mobile_header.y + 1,
            hits.mobile_switch.x.saturating_sub(layout.mobile_header.x),
            1,
        );
    }
    if !tab_area.is_empty() {
        render_tab_bar(
            buffer,
            tab_area,
            &bar,
            config,
            mobile_pane_bar,
            state.tab_scroll,
            state.reveal_focused_tab,
            state.tab_drag_insert_index,
            &mut hits,
        );
    }
    hits
}

pub(super) fn put_right_text(buffer: &mut Buffer, area: Rect, y: u16, text: &str, style: Style) {
    let width = display_width(text).min(area.width);
    put_text(
        buffer,
        area.right().saturating_sub(width),
        y,
        width,
        text,
        style,
    );
}

pub(super) fn put_segment(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    right: u16,
    text: &str,
    style: Style,
) -> u16 {
    let width = display_width(text).min(right.saturating_sub(x));
    put_text(buffer, x, y, width, text, style);
    x.saturating_add(width)
}

pub(super) fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if width == 0 || y >= buffer.area.bottom() || x >= buffer.area.right() {
        return;
    }
    buffer.set_stringn(x, y, text, width as usize, style);
}

pub(super) fn display_width(text: &str) -> u16 {
    UnicodeWidthStr::width(text).min(u16::MAX as usize) as u16
}
