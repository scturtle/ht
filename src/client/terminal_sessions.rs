use crate::protocol::{
    AttachScrollDirection, AttachScrollSource, ClientMessage, ClientMouseButton, ClientMouseKind,
    ClientMousePosition,
};
use base64::Engine;
#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum TerminalControlCommand {
    #[serde(rename = "terminal.input")]
    Input {
        text: Option<String>,
        bytes: Option<String>,
    },
    #[serde(rename = "terminal.resize")]
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        cell_width_px: u32,
        #[serde(default)]
        cell_height_px: u32,
    },
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: TerminalControlScrollDirection,
        lines: u16,
        #[serde(default)]
        source: TerminalControlScrollSource,
        #[serde(default)]
        column: Option<u16>,
        #[serde(default)]
        row: Option<u16>,
        #[serde(default)]
        modifiers: u8,
    },
    #[serde(rename = "terminal.mouse")]
    Mouse {
        action: TerminalControlMouseAction,
        #[serde(default)]
        button: TerminalControlMouseButton,
        column: u16,
        row: u16,
        #[serde(default)]
        modifiers: u8,
    },
    #[serde(rename = "terminal.release")]
    Release {},
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlMouseAction {
    Down,
    Up,
    Drag,
    Move,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlMouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlScrollSource {
    #[default]
    Wheel,
    PageKey,
}

pub(super) fn terminal_control_command_from_json(raw: &str) -> Result<ClientMessage, String> {
    let command = serde_json::from_str::<TerminalControlCommand>(raw)
        .map_err(|err| format!("invalid json command: {err}"))?;
    match command {
        TerminalControlCommand::Input { text, bytes } => {
            let data = match (text, bytes) {
                (Some(_), Some(_)) => {
                    return Err("terminal.input accepts text or bytes, not both".into())
                }
                (Some(text), None) => text.into_bytes(),
                (None, Some(bytes)) => base64::engine::general_purpose::STANDARD
                    .decode(bytes)
                    .map_err(|err| format!("invalid terminal.input bytes: {err}"))?,
                (None, None) => Vec::new(),
            };
            Ok(ClientMessage::Input { data })
        }
        TerminalControlCommand::Resize {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
        } => {
            if cols == 0 || rows == 0 {
                return Err("terminal.resize cols and rows must be greater than 0".into());
            }
            Ok(ClientMessage::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse: false,
            })
        }
        TerminalControlCommand::Scroll {
            direction,
            lines,
            source,
            column,
            row,
            modifiers,
        } => {
            if lines == 0 {
                return Err("terminal.scroll lines must be greater than 0".into());
            }
            let direction = match direction {
                TerminalControlScrollDirection::Up => AttachScrollDirection::Up,
                TerminalControlScrollDirection::Down => AttachScrollDirection::Down,
            };
            let source = match source {
                TerminalControlScrollSource::Wheel => AttachScrollSource::Wheel,
                TerminalControlScrollSource::PageKey => AttachScrollSource::PageKey {
                    input: match direction {
                        AttachScrollDirection::Up => b"\x1b[5~".to_vec(),
                        AttachScrollDirection::Down => b"\x1b[6~".to_vec(),
                    },
                },
            };
            Ok(ClientMessage::AttachScroll {
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            })
        }
        TerminalControlCommand::Mouse {
            action,
            button,
            column,
            row,
            modifiers,
        } => {
            let button = match button {
                TerminalControlMouseButton::Left => ClientMouseButton::Left,
                TerminalControlMouseButton::Right => ClientMouseButton::Right,
                TerminalControlMouseButton::Middle => ClientMouseButton::Middle,
            };
            let kind = match action {
                TerminalControlMouseAction::Down => ClientMouseKind::Down(button),
                TerminalControlMouseAction::Up => ClientMouseKind::Up(button),
                TerminalControlMouseAction::Drag => ClientMouseKind::Drag(button),
                TerminalControlMouseAction::Move => ClientMouseKind::Moved,
            };
            // The server encodes the event for the pane's mouse mode and drops it
            // when the application has not enabled mouse reporting.
            Ok(ClientMessage::AttachMouse {
                kind,
                position: ClientMousePosition::Cell { column, row },
                geometry: None,
                modifiers,
                lines: 1,
            })
        }
        TerminalControlCommand::Release {} => Ok(ClientMessage::Detach),
    }
}
