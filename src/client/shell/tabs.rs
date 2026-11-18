use super::*;
use crate::protocol::ClientShellPane;
use std::collections::HashMap;

const TAB_SCROLL_BUTTON_WIDTH: u16 = 3;
const MIN_TAB_STRIP_WIDTH: u16 =
    MIN_TAB_WIDTH + NEW_TAB_WIDTH + TAB_SCROLL_BUTTON_WIDTH.saturating_mul(2);

struct TabBarEntry<'a> {
    tab: &'a ClientShellTab,
    tab_index: usize,
    pane: Option<(usize, &'a ClientShellPane)>,
}

impl TabBarEntry<'_> {
    fn focused(&self) -> bool {
        self.pane.map_or(self.tab.focused, |(_, pane)| pane.focused)
    }

    fn label(&self, compact: bool) -> String {
        if let Some((number, pane)) = self.pane {
            let mut label = format!("{}.{number}", self.tab_index + 1);
            if !compact {
                let name = pane.label.as_deref().or_else(|| {
                    (number == 1 && self.tab.custom_label).then_some(self.tab.label.as_str())
                });
                if let Some(name) = name {
                    label.push(' ');
                    label.push_str(name);
                }
            }
            label
        } else if compact {
            (self.tab_index + 1).to_string()
        } else {
            self.tab.label.clone()
        }
    }
}

pub(in crate::client::shell) struct TabBarEntries<'a> {
    entries: Vec<TabBarEntry<'a>>,
    tab_count: usize,
    grouped: bool,
}

impl<'a> TabBarEntries<'a> {
    pub(in crate::client::shell) fn new(snapshot: &'a ClientShellSnapshot) -> Self {
        let mut panes = HashMap::<&str, Vec<_>>::new();
        let workspace = snapshot.focused_workspace_id.as_deref();
        for pane in &snapshot.panes {
            if Some(pane.workspace_id.as_str()) == workspace {
                panes.entry(&pane.tab_id).or_default().push(pane);
            }
        }
        let mut bar = Self {
            entries: Vec::new(),
            tab_count: 0,
            grouped: false,
        };
        for tab in &snapshot.tabs {
            if Some(tab.workspace_id.as_str()) != workspace {
                continue;
            }
            let tab_index = bar.tab_count;
            bar.tab_count += 1;
            match panes.get(tab.tab_id.as_str()) {
                Some(group) if group.len() > 1 => {
                    bar.grouped = true;
                    bar.entries
                        .extend(group.iter().enumerate().map(|(index, pane)| TabBarEntry {
                            tab,
                            tab_index,
                            pane: Some((index + 1, *pane)),
                        }));
                }
                _ => bar.entries.push(TabBarEntry {
                    tab,
                    tab_index,
                    pane: None,
                }),
            }
        }
        bar
    }
}

pub(in crate::client::shell) fn render_tab_bar(
    buffer: &mut Buffer,
    area: Rect,
    bar: &TabBarEntries<'_>,
    config: &ClientShellConfig,
    narrow_header: bool,
    tab_scroll: &mut usize,
    reveal_focused_tab: &mut bool,
    tab_drag_insert_index: Option<usize>,
    hits: &mut ShellHitMap,
) {
    if area.is_empty() {
        return;
    }
    let palette = &config.palette;
    buffer.set_style(area, Style::default().bg(palette.panel_bg));
    let measure = |compact| {
        let labels = bar
            .entries
            .iter()
            .map(|entry| entry.label(compact))
            .collect::<Vec<_>>();
        let widths = labels
            .iter()
            .map(|label| {
                if compact {
                    display_width(label)
                } else {
                    display_width(label).saturating_add(4).max(MIN_TAB_WIDTH)
                }
            })
            .collect::<Vec<_>>();
        (labels, widths)
    };
    let total = |widths: &[u16], new_tab_width: u16| {
        widths
            .iter()
            .copied()
            .fold(0_u16, u16::saturating_add)
            .saturating_add(widths.len().saturating_sub(1).min(u16::MAX as usize) as u16)
            .saturating_add(new_tab_width)
    };
    let (mut labels, mut desired_widths) = measure(false);
    let compact =
        (bar.grouped || narrow_header) && total(&desired_widths, NEW_TAB_WIDTH) > area.width;
    if compact {
        (labels, desired_widths) = measure(true);
    }
    let new_tab_width = if compact { 1 } else { NEW_TAB_WIDTH };
    let arrow_width = if compact { 1 } else { TAB_SCROLL_BUTTON_WIDTH };
    let needs_scroll = total(&desired_widths, new_tab_width) > area.width;
    let overflow = needs_scroll && area.width >= if compact { 4 } else { MIN_TAB_STRIP_WIDTH };
    let available = area
        .width
        .saturating_sub(new_tab_width + if overflow { 2 * arrow_width } else { 0 });
    let max_scroll = max_tab_scroll(&desired_widths, available);
    if !needs_scroll || (!overflow && !compact) {
        *tab_scroll = 0;
    } else if *reveal_focused_tab {
        if let Some(focused) = bar.entries.iter().position(TabBarEntry::focused) {
            *tab_scroll = centered_tab_scroll(focused, &desired_widths, available).min(max_scroll);
        }
    }
    *tab_scroll = (*tab_scroll).min(max_scroll);
    *reveal_focused_tab = false;

    let content = area;
    let mut x = content.x;
    let tab_right = if overflow {
        hits.tab_scroll_left = Rect::new(content.x, area.y, arrow_width, 1);
        put_text(
            buffer,
            hits.tab_scroll_left.x,
            area.y,
            hits.tab_scroll_left.width,
            if compact { "<" } else { " < " },
            Style::default()
                .fg(if *tab_scroll > 0 {
                    palette.overlay1
                } else {
                    palette.overlay0
                })
                .bg(palette.surface0),
        );
        x = hits.tab_scroll_left.right();
        content.right().saturating_sub(new_tab_width + arrow_width)
    } else {
        content.right().saturating_sub(new_tab_width)
    };

    let mut first_visible = None;
    let mut last_visible = None;
    for (index, entry) in bar.entries.iter().enumerate().skip(*tab_scroll) {
        let name = &labels[index];
        let desired = desired_widths[index];
        let remaining = tab_right.saturating_sub(x);
        let width = desired.min(remaining);
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        let named = entry
            .pane
            .map_or(entry.tab.custom_label, |(_, pane)| pane.label.is_some());
        let style = if entry.focused() {
            let base = Style::default()
                .fg(panel_contrast_fg(palette))
                .bg(palette.accent);
            if named {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            }
        } else {
            Style::default()
                .fg(if named {
                    palette.overlay1
                } else {
                    palette.overlay0
                })
                .bg(palette.surface0)
        };
        let padding = usize::from(width.saturating_sub(display_width(name)));
        let left = padding / 2;
        let right = padding - left;
        let text = format!("{:left$}{name}{:right$}", "", "");
        put_text(buffer, rect.x, rect.y, rect.width, &text, style);
        if let Some((_, pane)) = entry.pane {
            hits.tab_panes.push((rect, pane.pane_id.clone()));
        }
        if let Some((group, _)) = hits
            .tabs
            .last_mut()
            .filter(|(_, id)| id == &entry.tab.tab_id)
        {
            group.width = rect.right().saturating_sub(group.x);
        } else {
            if bar.grouped && !hits.tabs.is_empty() {
                put_text(
                    buffer,
                    x - 1,
                    area.y,
                    1,
                    "│",
                    Style::default().fg(palette.overlay0),
                );
            }
            hits.tabs.push((rect, entry.tab.tab_id.clone()));
        }
        first_visible.get_or_insert(index);
        last_visible = Some(index);
        x = x.saturating_add(width + 1);
        if width < desired {
            break;
        }
    }

    if overflow {
        hits.tab_scroll_right = Rect::new(tab_right, area.y, arrow_width, 1);
        let can_scroll_right = *tab_scroll < max_scroll;
        put_text(
            buffer,
            hits.tab_scroll_right.x,
            area.y,
            hits.tab_scroll_right.width,
            if compact { ">" } else { " > " },
            Style::default()
                .fg(if can_scroll_right {
                    palette.overlay1
                } else {
                    palette.overlay0
                })
                .bg(palette.surface0),
        );
        hits.new_tab = Rect::new(
            hits.tab_scroll_right.right(),
            area.y,
            content
                .right()
                .saturating_sub(hits.tab_scroll_right.right())
                .min(new_tab_width),
            1,
        );
    } else {
        hits.new_tab = Rect::new(
            x.min(content.right()),
            area.y,
            content.right().saturating_sub(x).min(new_tab_width),
            1,
        );
    }
    put_text(
        buffer,
        hits.new_tab.x,
        area.y,
        hits.new_tab.width,
        if compact { "+" } else { " + " },
        Style::default().fg(palette.overlay1).bg(palette.panel_bg),
    );

    if !compact && !bar.grouped && first_visible.is_some_and(|index| index > 0) {
        let ellipsis_x = if hits.tab_scroll_left.width > 0 {
            hits.tab_scroll_left.right()
        } else {
            content.x
        };
        put_text(
            buffer,
            ellipsis_x,
            area.y,
            u16::from(ellipsis_x < content.right()),
            "…",
            Style::default().fg(palette.overlay0),
        );
    }
    if !compact && !bar.grouped && last_visible.is_some_and(|index| index + 1 < bar.entries.len()) {
        let ellipsis_x = if hits.tab_scroll_right.width > 0 {
            hits.tab_scroll_right.x.saturating_sub(1)
        } else {
            content.right().saturating_sub(1)
        };
        put_text(
            buffer,
            ellipsis_x,
            area.y,
            u16::from(ellipsis_x >= content.x && ellipsis_x < content.right()),
            "…",
            Style::default().fg(palette.overlay0),
        );
    }

    if let Some(insert_index) = tab_drag_insert_index {
        if let Some(indicator_x) = tab_drop_indicator_x(hits, bar, insert_index) {
            put_text(
                buffer,
                indicator_x.min(content.right().saturating_sub(1)),
                area.y,
                1,
                "│",
                Style::default().fg(palette.accent),
            );
        }
    }
}

fn tab_drop_indicator_x(
    hits: &ShellHitMap,
    bar: &TabBarEntries<'_>,
    insert_index: usize,
) -> Option<u16> {
    let visible = hits
        .tabs
        .iter()
        .filter_map(|(rect, tab_id)| {
            bar.entries
                .iter()
                .find(|entry| entry.tab.tab_id == *tab_id)
                .map(|entry| (entry.tab_index, *rect))
        })
        .collect::<Vec<_>>();
    let (first_index, first_rect) = *visible.first()?;
    let (last_index, last_rect) = *visible.last()?;
    if insert_index == 0 {
        return Some(if first_index == 0 {
            first_rect.x
        } else {
            hits.tab_scroll_left.right()
        });
    }
    if let Some((_, rect)) = visible.iter().find(|(index, _)| *index == insert_index) {
        return Some(rect.x.saturating_sub(1));
    }
    if insert_index >= bar.tab_count {
        return Some(if last_index + 1 >= bar.tab_count {
            last_rect.right()
        } else {
            hits.tab_scroll_right.x.saturating_sub(1)
        });
    }
    None
}

fn centered_tab_scroll(focused: usize, widths: &[u16], available: u16) -> usize {
    let available = u32::from(available);
    let focused_width = u32::from(widths[focused]).min(available);
    let mut best = focused;
    let mut best_distance = available - focused_width;
    let mut before = 0;
    for start in (0..focused).rev() {
        before += u32::from(widths[start]) + 1;
        if before + focused_width > available {
            break;
        }
        let distance = (before * 2 + focused_width).abs_diff(available);
        if distance < best_distance {
            best_distance = distance;
            best = start;
        }
    }
    best
}

fn max_tab_scroll(widths: &[u16], available: u16) -> usize {
    let Some((&last, preceding)) = widths.split_last() else {
        return 0;
    };
    let mut start = preceding.len();
    let mut used = u32::from(last);
    // Keep the longest fully visible suffix, not merely a sliver of the last tab.
    // An oversized last tab must still be reachable at the start of the strip.
    for width in preceding.iter().rev() {
        let required = used + 1 + u32::from(*width);
        if required > u32::from(available) {
            break;
        }
        used = required;
        start -= 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::max_tab_scroll;

    #[test]
    fn trailing_scroll_limit_accounts_for_full_widths_and_separators() {
        for (widths, available, expected) in [
            (&[][..], 0, 0),
            (&[8, 13][..], 0, 1),
            (&[8, 13][..], 1, 1),
            (&[8, 13][..], 12, 1),
            (&[8, 13][..], 21, 1),
            (&[8, 13][..], 22, 0),
            (&[8, 13][..], 30, 0),
            (&[8, u16::MAX][..], u16::MAX, 1),
        ] {
            assert_eq!(
                max_tab_scroll(widths, available),
                expected,
                "widths={widths:?}, available={available}"
            );
        }
    }
}
