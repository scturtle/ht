//! Ordered native, default-color and legacy C1 replies from one PTY write.
//! Keep chunk offsets and native-reply suppression together; never reorder replies.

use bytes::Bytes;

use super::{GhosttyPaneCore, GhosttyPaneTerminal};
use crate::pane::{
    osc::{DefaultColorEvent, DefaultColorQuery, DefaultColorTrackedEvent},
    xtgettcap::C1XtgettcapResponse,
};

impl GhosttyPaneTerminal {
    pub(super) fn write_pty_bytes_with_ordered_responses(
        &self,
        core: &mut GhosttyPaneCore,
        bytes: &[u8],
        default_color_events: Vec<DefaultColorTrackedEvent>,
        in_progress_default_color_event: Option<DefaultColorEvent>,
        c1_xtgettcap_responses: Vec<C1XtgettcapResponse>,
        terminal_responses: &mut Vec<Bytes>,
    ) {
        // Only legacy C1 replies need merging; ordinary XTGETTCAP remains native.
        let mut events: Vec<_> = default_color_events
            .into_iter()
            .map(OrderedColorOrC1Event::Color)
            .chain(
                c1_xtgettcap_responses
                    .into_iter()
                    .map(OrderedColorOrC1Event::C1),
            )
            .collect();
        events.sort_by_key(OrderedColorOrC1Event::end_offset);
        let mut written = 0;
        for event in events {
            let end_offset = event.end_offset().min(bytes.len());
            if matches!(&event, OrderedColorOrC1Event::C1(response) if response.suppress_native) {
                // Suppress only the unhook byte's replies, not earlier native queries.
                let prefix_end = end_offset.saturating_sub(1).max(written);
                if prefix_end > written {
                    core.terminal.write(&bytes[written..prefix_end]);
                    terminal_responses.extend(self.drain_pending_pty_responses());
                    written = prefix_end;
                }
            }
            let mut libghostty_responses = Vec::new();
            if end_offset > written {
                core.terminal.write(&bytes[written..end_offset]);
                libghostty_responses = self.drain_pending_pty_responses();
                written = end_offset;
            }
            match event {
                OrderedColorOrC1Event::Color(event) => {
                    let replacement = respond_to_default_color_event(core, event.event);
                    if replacement.is_some() {
                        remove_last_matching_libghostty_color_reply(
                            &mut libghostty_responses,
                            event.event,
                        );
                    }
                    terminal_responses.extend(libghostty_responses);
                    terminal_responses.extend(replacement);
                }
                OrderedColorOrC1Event::C1(response) => {
                    if response.suppress_native {
                        libghostty_responses.retain(|reply| {
                            !reply.starts_with(b"\x1bP1+r") && !reply.starts_with(b"\x1bP0+r")
                        });
                    }
                    terminal_responses.extend(libghostty_responses);
                    if !response.suppress_native {
                        terminal_responses.push(response.bytes);
                    }
                }
            }
        }

        if written < bytes.len() {
            core.terminal.write(&bytes[written..]);
            let mut libghostty_responses = self.drain_pending_pty_responses();
            if let Some(event) = in_progress_default_color_event {
                if default_color_event_response(core, event).is_some() {
                    remove_last_matching_libghostty_color_reply(&mut libghostty_responses, event);
                }
            }
            terminal_responses.extend(libghostty_responses);
        }

        if !core.theme.child_foreground_changed && !core.theme.child_background_changed {
            core.theme.transient_owner_pgid = None;
        }
    }

    pub(super) fn drain_pending_pty_responses(&self) -> Vec<Bytes> {
        self.pending_pty_responses
            .lock()
            .map(|mut responses| std::mem::take(&mut *responses))
            .unwrap_or_default()
    }
}

enum OrderedColorOrC1Event {
    Color(DefaultColorTrackedEvent),
    C1(C1XtgettcapResponse),
}

impl OrderedColorOrC1Event {
    fn end_offset(&self) -> usize {
        match self {
            Self::Color(event) => event.end_offset,
            Self::C1(response) => response.end_offset,
        }
    }
}

fn remove_last_matching_libghostty_color_reply(
    responses: &mut Vec<Bytes>,
    event: DefaultColorEvent,
) {
    if let Some(index) = responses
        .iter()
        .rposition(|response| is_matching_libghostty_color_reply(response, event))
    {
        responses.remove(index);
    }
}

fn is_matching_libghostty_color_reply(response: &Bytes, event: DefaultColorEvent) -> bool {
    let prefix = match event {
        DefaultColorEvent::Query(query) => format!("\x1b]{};rgb:", query.osc_number()),
        DefaultColorEvent::PaletteQuery(index) => format!("\x1b]4;{index};rgb:"),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => return false,
    };
    response.starts_with(prefix.as_bytes())
        && (response.ends_with(b"\x07") || response.ends_with(b"\x1b\\"))
}

fn respond_to_default_color_event(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
) -> Option<Bytes> {
    match event {
        DefaultColorEvent::Query(_) | DefaultColorEvent::PaletteQuery(_) => {
            default_color_event_response(core, event)
        }
        DefaultColorEvent::Set(query) => {
            core.theme.mark_child_default_color_changed(query, true);
            None
        }
        DefaultColorEvent::Reset(query) => {
            core.theme.mark_child_default_color_changed(query, false);
            core.theme
                .apply_cached_host_default_color(&mut core.terminal, query);
            None
        }
    }
}

fn default_color_event_response(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
) -> Option<Bytes> {
    match event {
        DefaultColorEvent::Query(query) => default_color_query_response(query, core),
        DefaultColorEvent::PaletteQuery(index) => palette_color_query_response(index, core),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => None,
    }
}

fn default_color_query_response(
    query: DefaultColorQuery,
    core: &mut GhosttyPaneCore,
) -> Option<Bytes> {
    let color = match query {
        DefaultColorQuery::Foreground if !core.theme.child_foreground_changed => {
            core.theme.host.foreground.map(host_theme_color_to_ghostty)
        }
        DefaultColorQuery::Background if !core.theme.child_background_changed => {
            core.theme.host.background.map(host_theme_color_to_ghostty)
        }
        DefaultColorQuery::Cursor => cursor_color_query_color(core),
        _ => None,
    }?;
    Some(osc_rgb_response(
        &query.osc_number().to_string(),
        color.r,
        color.g,
        color.b,
    ))
}

fn cursor_color_query_color(core: &mut GhosttyPaneCore) -> Option<crate::ghostty::RgbColor> {
    let host_foreground = core.theme.host.foreground;
    let child_foreground_changed = core.theme.child_foreground_changed;
    core.terminal
        .effective_cursor_color()
        .ok()
        .flatten()
        .or_else(|| {
            if child_foreground_changed {
                core.terminal.effective_foreground_color().ok().flatten()
            } else {
                host_foreground
                    .map(host_theme_color_to_ghostty)
                    .or_else(|| core.terminal.effective_foreground_color().ok().flatten())
            }
        })
}

fn palette_color_query_response(index: u8, core: &mut GhosttyPaneCore) -> Option<Bytes> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal).ok()?;
    let colors = render_state.colors().ok()?;
    let color = colors.palette[usize::from(index)];
    Some(osc_rgb_response(
        &format!("4;{index}"),
        color.r,
        color.g,
        color.b,
    ))
}

fn osc_rgb_response(command: &str, r: u8, g: u8, b: u8) -> Bytes {
    let r = u16::from(r) * 257;
    let g = u16::from(g) * 257;
    let b = u16::from(b) * 257;
    Bytes::from(format!("\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}\x1b\\"))
}

fn host_theme_color_to_ghostty(color: crate::terminal_theme::RgbColor) -> crate::ghostty::RgbColor {
    crate::ghostty::RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}
