//! Same semantic input and Ghostty encoders as pane input, without a PTY writer.
use super::view::View;
use crate::protocol::{ClientMousePosition, ClientPaneInputEvent};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

/// A held gesture belongs to its original target, even after that target closes.
/// Its continuation/cleanup must neither error nor be rerouted to a survivor.
pub(super) fn continuation_only(events: &[ClientPaneInputEvent]) -> bool {
    events.iter().all(|event| {
        matches!(
            event,
            ClientPaneInputEvent::Key {
                kind: crate::protocol::ClientKeyKind::Release
                    | crate::protocol::ClientKeyKind::Repeat,
                ..
            } | ClientPaneInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Up(_)
                    | crate::protocol::ClientMouseKind::Drag(_)
                    | crate::protocol::ClientMouseKind::Moved,
                ..
            }
        )
    })
}

pub(super) fn rejection(
    events: &[ClientPaneInputEvent],
    reason: &str,
) -> crate::protocol::ServerMessage {
    let action = if events
        .iter()
        .any(|event| matches!(event, ClientPaneInputEvent::Paste(_)))
    {
        "Paste"
    } else {
        "Input"
    };
    crate::protocol::ServerMessage::ClientShellError {
        message: format!("{action} rejected: {reason}"),
    }
}

pub(super) fn for_target(
    model: &super::model::Model,
    views: &std::collections::HashMap<crate::layout::PaneId, View>,
    address: &str,
    events: &[ClientPaneInputEvent],
) -> std::io::Result<Option<(u32, Vec<u8>)>> {
    let id = model.id(address);
    let view = id.and_then(|id| views.get(&id)).filter(|view| !view.closed);
    let Some(view) = view else {
        if continuation_only(events) {
            return Ok(None);
        }
        return Err(super::model::invalid(if id.is_none() {
            "Core pane not found"
        } else {
            "Core program has finished"
        }));
    };
    let data = encode(view, events);
    Ok((!data.is_empty()).then_some((view.channel, data)))
}

pub(in crate::client) fn encode(view: &View, events: &[ClientPaneInputEvent]) -> Vec<u8> {
    let terminal = &view.terminal;
    let mut output = Vec::new();
    for event in events {
        if let ClientPaneInputEvent::Mouse {
            kind,
            position,
            geometry,
            modifiers,
            lines,
        } = event
        {
            let kind = kind.to_crossterm();
            let modifiers = KeyModifiers::from_bits_truncate(*modifiers);
            let position = match *position {
                ClientMousePosition::Cell { column, row } => {
                    crate::input::mouse::Position::Cell { column, row }
                }
                ClientMousePosition::Pixels { x, y, column, row } => {
                    let exact = terminal.sgr_pixel_mouse_enabled()
                        && geometry.is_some_and(|g| {
                            (g.cols, g.rows) == (view.size.0, view.size.1)
                                && (g.width_px, g.height_px)
                                    == (
                                        u32::from(view.size.0) * view.size.2,
                                        u32::from(view.size.1) * view.size.3,
                                    )
                                && x > 0
                                && y > 0
                                && x <= g.width_px
                                && y <= g.height_px
                        });
                    if exact {
                        crate::input::mouse::Position::Pixels { x, y }
                    } else {
                        crate::input::mouse::Position::Cell { column, row }
                    }
                }
            };
            if matches!(kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                match terminal.wheel_routing() {
                    Some(crate::pane::WheelRouting::MouseReport) => {
                        terminal.scroll_reset();
                        if let Some(bytes) = terminal.encode_mouse_wheel(kind, position, modifiers)
                        {
                            output.extend(bytes);
                        }
                    }
                    Some(crate::pane::WheelRouting::AlternateScroll) => {
                        terminal.scroll_reset();
                        let key = if kind == MouseEventKind::ScrollUp {
                            KeyCode::Up
                        } else {
                            KeyCode::Down
                        };
                        output.extend(terminal.encode_terminal_key(
                            crate::input::TerminalKey::new(key, KeyModifiers::empty()),
                        ));
                    }
                    _ => {
                        if kind == MouseEventKind::ScrollUp {
                            terminal.scroll_up(usize::from((*lines).max(1)))
                        } else {
                            terminal.scroll_down(usize::from((*lines).max(1)))
                        }
                    }
                }
                continue;
            }
            let bytes = match kind {
                MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => {
                    terminal.encode_mouse_button(kind, position, modifiers)
                }
                MouseEventKind::Moved => terminal.encode_mouse_motion(kind, position, modifiers),
                _ => terminal.encode_mouse_wheel(kind, position, modifiers),
            };
            if let Some(bytes) = bytes {
                if kind != MouseEventKind::Moved {
                    terminal.scroll_reset();
                }
                output.extend(bytes);
            }
            continue;
        }
        match event.to_raw_input_event() {
            crate::raw_input::RawInputEvent::Key(key) => {
                if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                    && key.modifiers.is_empty()
                    && terminal.plain_page_keys_use_host_scrollback() == Some(true)
                {
                    if key.kind != KeyEventKind::Release {
                        if key.code == KeyCode::PageUp {
                            terminal.scroll_up(usize::from(view.size.1))
                        } else {
                            terminal.scroll_down(usize::from(view.size.1))
                        }
                    }
                } else {
                    terminal.scroll_reset();
                    output.extend(terminal.encode_terminal_key(key));
                }
            }
            crate::raw_input::RawInputEvent::Text(text) => {
                terminal.scroll_reset();
                output.extend_from_slice(text.as_str().as_bytes());
            }
            crate::raw_input::RawInputEvent::Paste(text) => {
                terminal.scroll_reset();
                let text = crate::platform::prepare_paste_text_for_pty(text);
                if terminal.bracketed_paste_enabled() {
                    output.extend_from_slice(b"\x1b[200~");
                }
                output.extend_from_slice(text.as_bytes());
                if terminal.bracketed_paste_enabled() {
                    output.extend_from_slice(b"\x1b[201~");
                }
            }
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PaneId;
    #[test]
    fn semantic_keys_and_paste_follow_the_programs_negotiated_modes() {
        let view = View::new(80, 24, 1).unwrap();
        let key = |code| {
            ClientPaneInputEvent::from_terminal_key(crate::input::TerminalKey::new(
                code,
                KeyModifiers::empty(),
            ))
            .unwrap()
        };
        assert_eq!(encode(&view, &[key(KeyCode::Up)]), b"\x1b[A");
        view.terminal
            .process_pty_bytes(PaneId::alloc(), b"\x1b[?1h\x1b[?2004h");
        assert_eq!(encode(&view, &[key(KeyCode::Up)]), b"\x1bOA");
        assert_eq!(
            encode(&view, &[ClientPaneInputEvent::Paste("hello".into())]),
            b"\x1b[200~hello\x1b[201~"
        );
    }
    #[test]
    fn closed_target_continuations_are_noops_but_new_input_is_not_redirected() {
        use crate::protocol::{ClientKeyKind, ClientMouseButton, ClientMouseKind};
        let mut model = super::super::model::Model::new("cleanup".into());
        let old = model.insert(serde_json::json!({"address":"old"})).unwrap();
        let survivor = model
            .insert(serde_json::json!({"address":"survivor"}))
            .unwrap();
        let mut views = std::collections::HashMap::new();
        let live = View::new(80, 24, 22).unwrap();
        live.terminal
            .process_pty_bytes(survivor, b"\x1b[>31u\x1b[?1000h\x1b[?1006h");
        views.insert(survivor, live);
        model.retire(old).unwrap();
        let key = ClientPaneInputEvent::from_terminal_key(
            crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::empty())
                .with_kind(KeyEventKind::Release),
        )
        .unwrap();
        let mouse = ClientPaneInputEvent::Mouse {
            kind: ClientMouseKind::Up(ClientMouseButton::Left),
            position: ClientMousePosition::Cell { column: 1, row: 1 },
            geometry: None,
            modifiers: 0,
            lines: 1,
        };
        assert!(for_target(&model, &views, "old", &[key.clone(), mouse])
            .unwrap()
            .is_none());
        assert_eq!(
            for_target(&model, &views, "survivor", std::slice::from_ref(&key))
                .unwrap()
                .unwrap()
                .0,
            22
        );
        let mut repeat = key.clone();
        if let ClientPaneInputEvent::Key { kind, .. } = &mut repeat {
            *kind = ClientKeyKind::Repeat;
        }
        assert!(for_target(&model, &views, "old", &[repeat])
            .unwrap()
            .is_none());
        let paste = ClientPaneInputEvent::Paste("do-not-replay".into());
        assert!(for_target(&model, &views, "old", std::slice::from_ref(&paste)).is_err());
        assert!(for_target(&model, &views, "old", &[key, paste]).is_err());
        views.get_mut(&survivor).unwrap().closed = true;
        assert!(for_target(
            &model,
            &views,
            "survivor",
            &[ClientPaneInputEvent::TextCommit("new input".into())]
        )
        .is_err());
        assert_eq!(model.focused(), Some(survivor));
    }

    #[test]
    fn terminal_query_responses_are_separate_from_human_input() {
        let view = View::new(80, 24, 1).unwrap();
        let result = view.terminal.process_pty_bytes(PaneId::alloc(), b"\x1b[6n");
        assert!(!result.terminal_responses.is_empty());
        assert!(encode(&view, &[]).is_empty());
    }
}
