//! In-process client/Core adapter messages and shared presentation data.
//! Core's actual channel framing lives in client/core/transport.rs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Control/presentation reserve used when budgeting graphics publication.
pub const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

/// Total surface publication budget including graphics; not a socket frame limit.
pub const MAX_GRAPHICS_FRAME_SIZE: usize = 32 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Client → Server messages
// ---------------------------------------------------------------------------

/// Size of the pane surface requested by a client-owned shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSurfaceSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    Char(char),
    F(u8),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMouseKind {
    Down(ClientMouseButton),
    Up(ClientMouseButton),
    Drag(ClientMouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMousePosition {
    Cell {
        column: u16,
        row: u16,
    },
    Pixels {
        x: u32,
        y: u32,
        column: u16,
        row: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMouseGeometry {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientInputEvent {
    Key {
        code: ClientKeyCode,
        modifiers: u8,
        kind: ClientKeyKind,
        repeat_count: u16,
        generated_text: Option<String>,
        source: ClientKeySource,
    },
    TextCommit(String),
    Mouse {
        kind: ClientMouseKind,
        column: u16,
        row: u16,
        modifiers: u8,
    },
    Paste {
        text: String,
    },
    FocusGained,
    FocusLost,
    HostDefaultColor {
        kind: ClientHostDefaultColorKind,
        color: ClientHostColor,
    },
}

/// Pane-domain input after the client has classified and consumed shell actions.
///
/// Keys are semantic rather than outer-terminal VT bytes so the target pane can
/// encode them for the child application's negotiated keyboard protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientPaneInputEvent {
    Key {
        code: ClientKeyCode,
        modifiers: u8,
        kind: ClientKeyKind,
        repeat_count: u16,
        shifted_codepoint: Option<u32>,
        generated_text: Option<String>,
        tracks_release: bool,
        physical_key_id: Option<u32>,
        windows_record: Option<crate::input::WindowsKeyRecord>,
    },
    TextCommit(String),
    Mouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
    Paste(String),
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKeySource {
    Synthesized,
    Vt {
        bytes: Vec<u8>,
    },
    WindowsConsole {
        record: crate::input::WindowsKeyRecord,
    },
}

impl ClientKeyKind {
    pub(crate) fn from_crossterm(kind: crossterm::event::KeyEventKind) -> Self {
        match kind {
            crossterm::event::KeyEventKind::Press => Self::Press,
            crossterm::event::KeyEventKind::Repeat => Self::Repeat,
            crossterm::event::KeyEventKind::Release => Self::Release,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::KeyEventKind {
        match self {
            Self::Press => crossterm::event::KeyEventKind::Press,
            Self::Repeat => crossterm::event::KeyEventKind::Repeat,
            Self::Release => crossterm::event::KeyEventKind::Release,
        }
    }
}

impl ClientKeyCode {
    pub(crate) fn from_crossterm(code: crossterm::event::KeyCode) -> Option<Self> {
        use crossterm::event::KeyCode;
        Some(match code {
            KeyCode::Backspace => Self::Backspace,
            KeyCode::Enter => Self::Enter,
            KeyCode::Left => Self::Left,
            KeyCode::Right => Self::Right,
            KeyCode::Up => Self::Up,
            KeyCode::Down => Self::Down,
            KeyCode::Home => Self::Home,
            KeyCode::End => Self::End,
            KeyCode::PageUp => Self::PageUp,
            KeyCode::PageDown => Self::PageDown,
            KeyCode::Tab => Self::Tab,
            KeyCode::BackTab => Self::BackTab,
            KeyCode::Delete => Self::Delete,
            KeyCode::Insert => Self::Insert,
            KeyCode::Esc => Self::Esc,
            KeyCode::Char(ch) => Self::Char(ch),
            KeyCode::F(n) => Self::F(n),
            KeyCode::Null => Self::Null,
            _ => return None,
        })
    }

    pub(crate) fn to_crossterm(&self) -> crossterm::event::KeyCode {
        use crossterm::event::KeyCode;
        match self {
            Self::Backspace => KeyCode::Backspace,
            Self::Enter => KeyCode::Enter,
            Self::Left => KeyCode::Left,
            Self::Right => KeyCode::Right,
            Self::Up => KeyCode::Up,
            Self::Down => KeyCode::Down,
            Self::Home => KeyCode::Home,
            Self::End => KeyCode::End,
            Self::PageUp => KeyCode::PageUp,
            Self::PageDown => KeyCode::PageDown,
            Self::Tab => KeyCode::Tab,
            Self::BackTab => KeyCode::BackTab,
            Self::Delete => KeyCode::Delete,
            Self::Insert => KeyCode::Insert,
            Self::Esc => KeyCode::Esc,
            Self::Char(ch) => KeyCode::Char(*ch),
            Self::F(n) => KeyCode::F(*n),
            Self::Null => KeyCode::Null,
        }
    }
}

impl ClientMouseButton {
    pub(crate) fn from_crossterm(button: crossterm::event::MouseButton) -> Self {
        match button {
            crossterm::event::MouseButton::Left => Self::Left,
            crossterm::event::MouseButton::Right => Self::Right,
            crossterm::event::MouseButton::Middle => Self::Middle,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseButton {
        match self {
            Self::Left => crossterm::event::MouseButton::Left,
            Self::Right => crossterm::event::MouseButton::Right,
            Self::Middle => crossterm::event::MouseButton::Middle,
        }
    }
}

impl ClientMouseKind {
    pub(crate) fn from_crossterm(kind: crossterm::event::MouseEventKind) -> Option<Self> {
        use crossterm::event::MouseEventKind;
        Some(match kind {
            MouseEventKind::Down(button) => Self::Down(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Up(button) => Self::Up(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Drag(button) => Self::Drag(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Moved => Self::Moved,
            MouseEventKind::ScrollUp => Self::ScrollUp,
            MouseEventKind::ScrollDown => Self::ScrollDown,
            MouseEventKind::ScrollLeft => Self::ScrollLeft,
            MouseEventKind::ScrollRight => Self::ScrollRight,
        })
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseEventKind {
        use crossterm::event::MouseEventKind;
        match self {
            Self::Down(button) => MouseEventKind::Down(button.to_crossterm()),
            Self::Up(button) => MouseEventKind::Up(button.to_crossterm()),
            Self::Drag(button) => MouseEventKind::Drag(button.to_crossterm()),
            Self::Moved => MouseEventKind::Moved,
            Self::ScrollUp => MouseEventKind::ScrollUp,
            Self::ScrollDown => MouseEventKind::ScrollDown,
            Self::ScrollLeft => MouseEventKind::ScrollLeft,
            Self::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

impl ClientPaneInputEvent {
    pub(crate) fn from_terminal_key(key: crate::input::TerminalKey) -> Option<Self> {
        let tracks_release = key.generated_text.is_none() || key.has_physical_identity();
        let physical_key_id = key.physical_key_id();
        let windows_record = key.windows_record();
        Some(Self::Key {
            code: ClientKeyCode::from_crossterm(key.code)?,
            modifiers: key.modifiers.bits(),
            kind: ClientKeyKind::from_crossterm(key.kind),
            repeat_count: key.repeat_count,
            shifted_codepoint: key.shifted_codepoint,
            generated_text: key.generated_text,
            tracks_release,
            physical_key_id,
            windows_record,
        })
    }

    pub(crate) fn to_raw_input_event(&self) -> crate::raw_input::RawInputEvent {
        self.to_raw_input_event_with_windows_source(cfg!(any(windows, test)))
    }

    fn to_raw_input_event_with_windows_source(
        &self,
        attach_windows_source: bool,
    ) -> crate::raw_input::RawInputEvent {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                shifted_codepoint,
                generated_text,
                tracks_release,
                windows_record,
                ..
            } => {
                let mut key = crate::input::TerminalKey::new(
                    code.to_crossterm(),
                    crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                )
                .with_kind(kind.to_crossterm())
                .with_repeat_count(*repeat_count)
                .with_generated_text(generated_text.clone())
                .with_physical_identity_hint(*tracks_release && generated_text.is_some())
                .with_windows_composition_hint(*windows_record);
                if let Some(shifted_codepoint) = shifted_codepoint {
                    key = key.with_shifted_codepoint(*shifted_codepoint);
                }
                #[cfg(test)]
                if attach_windows_source {
                    if let Some(record) = windows_record {
                        key = key.with_windows_record(*record);
                    }
                }
                let _ = (attach_windows_source, windows_record);
                crate::raw_input::RawInputEvent::Key(key)
            }
            Self::TextCommit(text) => {
                crate::raw_input::RawInputEvent::Text(crate::input::TextCommit::new(text.clone()))
            }
            Self::Mouse {
                kind,
                position,
                modifiers,
                ..
            } => {
                let (column, row) = match position {
                    ClientMousePosition::Cell { column, row }
                    | ClientMousePosition::Pixels { column, row, .. } => (*column, *row),
                };
                crate::raw_input::RawInputEvent::Mouse(crossterm::event::MouseEvent {
                    kind: kind.to_crossterm(),
                    column,
                    row,
                    modifiers: crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                })
            }
            Self::Paste(text) => crate::raw_input::RawInputEvent::Paste(text.clone()),
        }
    }
}

#[cfg(test)]
impl ClientInputEvent {
    pub(crate) fn to_raw_input_event(&self) -> crate::raw_input::RawInputEvent {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                generated_text,
                source,
            } => {
                let mut key = crate::input::TerminalKey::new(
                    code.to_crossterm(),
                    crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                )
                .with_generated_text(generated_text.clone());
                key = match source {
                    ClientKeySource::Synthesized => key,
                    ClientKeySource::Vt { bytes } => key.with_vt_bytes(bytes.clone()),
                    ClientKeySource::WindowsConsole { record } => key.with_windows_record(*record),
                };
                key = key
                    .with_repeat_count(*repeat_count)
                    .with_kind(kind.to_crossterm());
                crate::raw_input::RawInputEvent::Key(key)
            }
            Self::TextCommit(text) => {
                crate::raw_input::RawInputEvent::Text(crate::input::TextCommit::new(text.clone()))
            }
            Self::Mouse {
                kind,
                column,
                row,
                modifiers,
            } => crate::raw_input::RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind: kind.to_crossterm(),
                column: *column,
                row: *row,
                modifiers: crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
            }),
            Self::Paste { text } => crate::raw_input::RawInputEvent::Paste(text.clone()),
            Self::FocusGained => crate::raw_input::RawInputEvent::OuterFocusGained,
            Self::FocusLost => crate::raw_input::RawInputEvent::OuterFocusLost,
            Self::HostDefaultColor { kind, color } => {
                crate::raw_input::RawInputEvent::HostDefaultColor {
                    kind: match kind {
                        ClientHostDefaultColorKind::Foreground => {
                            crate::terminal_theme::DefaultColorKind::Foreground
                        }
                        ClientHostDefaultColorKind::Background => {
                            crate::terminal_theme::DefaultColorKind::Background
                        }
                    },
                    color: (*color).into(),
                }
            }
        }
    }
}

/// Messages sent from the client to the Core adapter carried as typed values.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    /// Raw input bytes read from the client's stdin.
    Input {
        /// Raw terminal input (possibly multi-byte escape sequences).
        data: Vec<u8>,
    },

    /// Image bytes read from the client's local clipboard for remote paste bridging.
    ClipboardImage {
        /// Stable terminal target selected by the client that read the clipboard.
        target: ClientClipboardImageTarget,
        /// Image file extension without a leading dot.
        extension: String,
        /// Raw image bytes.
        data: Vec<u8>,
    },

    /// Terminal resize notification from the client.
    Resize {
        /// New terminal width in columns.
        cols: u16,
        /// New terminal height in rows.
        rows: u16,
        /// Width of a terminal cell in physical pixels, or 0 when client-side Kitty graphics are disabled.
        cell_width_px: u32,
        /// Height of a terminal cell in physical pixels, or 0 when unavailable.
        cell_height_px: u32,
        /// Whether this resize carries coherent exact geometry for SGR pixel mouse input.
        pixel_mouse: bool,
    },

    /// Graceful disconnect request.
    Detach,

    /// Scroll input handled by a direct terminal attach client.
    AttachScroll {
        /// Original input source for routing.
        source: AttachScrollSource,
        /// Scroll direction.
        direction: AttachScrollDirection,
        /// Number of terminal rows to move when using host scrollback.
        lines: u16,
        /// Mouse column relative to the attached terminal, when available.
        column: Option<u16>,
        /// Mouse row relative to the attached terminal, when available.
        row: Option<u16>,
        /// Crossterm-compatible modifier bits for forwarded mouse wheel events.
        modifiers: u8,
    },

    /// Result of the one armed Herdr-owned direct Kitty transmission.
    GraphicsTransmissionResult {
        transfer_id: u64,
        image_id: u32,
        success: bool,
    },

    /// The direct command was written and flushed; terminal response timing starts now.
    GraphicsTransmissionStarted { transfer_id: u64, image_id: u32 },

    /// Resize the pane viewport of a client-owned shell.
    ClientShellResize {
        cell_width_px: u32,
        cell_height_px: u32,
        surface_size: ClientSurfaceSize,
        /// Whether this resize carries coherent exact geometry for SGR pixel mouse input.
        pixel_mouse: bool,
    },

    /// Deliver client-classified semantic input directly to a stable pane target.
    ClientShellPaneInput {
        pane_id: String,
        events: Vec<ClientPaneInputEvent>,
    },

    /// Invoke one endpoint operation through this client shell's selected connection.
    ///
    /// The request travels as a typed value: in-process transport carries no
    /// serialized API envelope between the shell and the Core adapter.
    ClientShellEndpointRequest {
        boot_id: String,
        request: Box<crate::api::schema::Request>,
    },

    /// Deliver one structured mouse event to a directly attached terminal.
    AttachMouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },

    /// Publish one host terminal color or appearance update observed by a client-owned shell.
    ClientShellHostTheme { update: ClientHostThemeUpdate },

    /// Publish whether the outer terminal containing a client shell has focus.
    ClientShellFocus { focused: bool },

    /// Update this client's shell mouse-capture preference after config reload.
    ClientShellMouseCapture { enabled: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHostColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<crate::terminal_theme::RgbColor> for ClientHostColor {
    fn from(color: crate::terminal_theme::RgbColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
        }
    }
}

impl From<ClientHostColor> for crate::terminal_theme::RgbColor {
    fn from(color: ClientHostColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostDefaultColorKind {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostAppearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostThemeUpdate {
    DefaultColor {
        kind: ClientHostDefaultColorKind,
        color: ClientHostColor,
    },
    PaletteColors(Vec<(u8, ClientHostColor)>),
    Appearance(ClientHostAppearance),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientClipboardImageTarget {
    DirectTerminal,
    Pane(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollSource {
    Wheel,
    PageKey {
        /// Original key bytes to forward when the child application owns page keys.
        input: Vec<u8>,
    },
}

// ---------------------------------------------------------------------------
// Server → Client messages
// ---------------------------------------------------------------------------

/// A single cell in a rendered frame, serialized independently from ratatui's
/// `Cell` type to keep the wire protocol stable.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellData {
    /// Grapheme cluster displayed in this cell (usually 1–2 chars).
    pub symbol: String,
    /// Foreground color as a packed u32 (0xAARRGGBB or ratatui Color index).
    pub fg: u32,
    /// Background color as a packed u32.
    pub bg: u32,
    /// Bitmask of style modifiers (bold, italic, etc.) plus Herdr extension bits.
    pub modifier: u16,
    /// Whether this cell should be skipped during diff-based rendering.
    pub skip: bool,
    /// Index into `FrameData::hyperlinks` for this cell's OSC 8 target, if any.
    pub hyperlink: Option<u32>,
}

impl Clone for CellData {
    fn clone(&self) -> Self {
        Self {
            symbol: self.symbol.clone(),
            ..*self
        }
    }

    fn clone_from(&mut self, source: &Self) {
        let mut symbol = std::mem::take(&mut self.symbol);
        symbol.clone_from(&source.symbol);
        *self = Self { symbol, ..*source };
    }
}

impl CellData {
    pub(crate) fn from_ratatui_cell(cell: &ratatui::buffer::Cell) -> Self {
        Self {
            symbol: cell.symbol().to_owned(),
            fg: color_to_u32(cell.fg),
            bg: color_to_u32(cell.bg),
            modifier: modifier_to_u16(cell.modifier),
            skip: cell.skip,
            hyperlink: None,
        }
    }
}

/// Cursor shape encoded as a DECSCUSR parameter.
///
/// 0 = terminal default, 1 = blinking block, 2 = steady block,
/// 3 = blinking underline, 4 = steady underline, 5 = blinking bar,
/// 6 = steady bar.
pub type CursorShapeParam = u8;

/// Cursor position within a rendered frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    /// Column offset (0-based) of the cursor.
    pub x: u16,
    /// Row offset (0-based) of the cursor.
    pub y: u16,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Cursor shape as a DECSCUSR parameter.
    #[serde(default)]
    pub shape: CursorShapeParam,
}

/// A rendered frame to be displayed by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameData {
    /// Cells in row-major order. Length must equal `width * height`.
    pub cells: Vec<CellData>,
    /// Frame width in columns.
    pub width: u16,
    /// Frame height in rows.
    pub height: u16,
    /// Cursor state for this frame, if applicable.
    pub cursor: Option<CursorState>,
    /// OSC 8 hyperlink URIs referenced by cells.
    pub hyperlinks: Vec<String>,
    /// Kitty graphics protocol bytes to apply after the text frame.
    pub graphics: Vec<u8>,
}

impl FrameData {
    /// Creates a `FrameData` from a ratatui `Buffer` and optional cursor.
    ///
    /// This converts ratatui's internal cell representation into the
    /// wire-protocol cell format. The conversion is lossless for all
    /// commonly used cell attributes.
    #[cfg(test)]
    pub fn from_ratatui_buffer(
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
    ) -> Self {
        Self::from_ratatui_buffer_with_hyperlinks(buffer, cursor, &[])
    }

    pub fn from_ratatui_buffer_with_hyperlinks(
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
        hyperlinks: &[((u16, u16), String, String)],
    ) -> Self {
        let area = buffer.area;
        let width = area.width;
        let height = area.height;

        let mut hyperlink_uris = Vec::<String>::new();
        let mut hyperlink_indices = HashMap::<&str, u32>::new();
        let mut hyperlink_by_position = HashMap::<(u16, u16), (&str, &str)>::new();
        for ((x, y), symbol, uri) in hyperlinks {
            hyperlink_by_position.insert((*x, *y), (symbol.as_str(), uri.as_str()));
        }
        let mut cells = Vec::with_capacity((width as usize) * (height as usize));
        for row in 0..height {
            for col in 0..width {
                let cell = buffer.cell((col, row)).expect("cell within bounds");
                let hyperlink = hyperlink_by_position
                    .get(&(col, row))
                    .and_then(|(symbol, uri)| {
                        if *symbol != cell.symbol() {
                            return None;
                        }
                        Some(*hyperlink_indices.entry(*uri).or_insert_with(|| {
                            let index = hyperlink_uris.len() as u32;
                            hyperlink_uris.push((*uri).to_owned());
                            index
                        }))
                    });
                let mut cell = CellData::from_ratatui_cell(cell);
                cell.hyperlink = hyperlink;
                cells.push(cell);
            }
        }

        FrameData {
            cells,
            width,
            height,
            cursor,
            hyperlinks: hyperlink_uris,
            graphics: Vec::new(),
        }
    }

    pub(crate) fn replace_from_ratatui_buffer_preserving_effects(
        &mut self,
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
    ) {
        let width = self.width;
        let hyperlinks = if width == 0 {
            Vec::new()
        } else {
            self.cells
                .iter()
                .enumerate()
                .filter_map(|(index, cell)| {
                    let uri = self.hyperlinks.get(cell.hyperlink? as usize)?;
                    let x = u16::try_from(index % usize::from(width)).ok()?;
                    let y = u16::try_from(index / usize::from(width)).ok()?;
                    Some(((x, y), cell.symbol.clone(), uri.clone()))
                })
                .collect::<Vec<_>>()
        };
        let graphics = std::mem::take(&mut self.graphics);
        let mut replacement =
            Self::from_ratatui_buffer_with_hyperlinks(buffer, cursor, &hyperlinks);
        replacement.graphics = graphics;
        *self = replacement;
    }

    /// Reconstructs a ratatui `Buffer` from this frame data.
    ///
    /// Returns `None` if the cells vector length doesn't match `width * height`.
    pub(crate) fn to_ratatui_buffer(&self) -> Option<ratatui::buffer::Buffer> {
        let expected = (self.width as usize) * (self.height as usize);
        if self.cells.len() != expected {
            return None;
        }

        let area = ratatui::layout::Rect::new(0, 0, self.width, self.height);
        let mut buffer = ratatui::buffer::Buffer::filled(area, ratatui::buffer::Cell::new(" "));

        for row in 0..self.height {
            for col in 0..self.width {
                let idx = (row as usize) * (self.width as usize) + (col as usize);
                let cell_data = &self.cells[idx];
                let cell = buffer.cell_mut((col, row)).expect("cell within bounds");
                cell.set_symbol(&cell_data.symbol);
                cell.fg = u32_to_color(cell_data.fg);
                cell.bg = u32_to_color(cell_data.bg);
                cell.modifier = u16_to_modifier(cell_data.modifier);
                cell.skip = cell_data.skip;
            }
        }

        Some(buffer)
    }
}

fn deserialize_client_shell_agent_status<'de, D>(
    deserializer: D,
) -> Result<crate::api::schema::AgentStatus, D::Error>
where
    D: serde::Deserializer<'de>,
{
    if !deserializer.is_human_readable() {
        return crate::api::schema::AgentStatus::deserialize(deserializer);
    }
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "idle" => crate::api::schema::AgentStatus::Idle,
        "working" => crate::api::schema::AgentStatus::Working,
        "blocked" => crate::api::schema::AgentStatus::Blocked,
        "done" => crate::api::schema::AgentStatus::Done,
        _ => crate::api::schema::AgentStatus::Unknown,
    })
}

/// Initial resource projection used by the stable client-owned shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellSnapshot {
    /// Changes whenever the adapter projection is rebuilt, including relay recovery.
    pub boot_id: String,
    /// Monotonic replacement revision within one endpoint boot.
    pub revision: u64,
    /// Endpoint startup/reload config warning, filtered for client-owned keybindings.
    pub config_diagnostic: Option<String>,
    /// Unseen announcement owned and persisted by this endpoint.
    pub product_announcement: Option<ClientShellProductAnnouncement>,
    /// Future-version update advertised by the endpoint.
    pub update_available: Option<String>,
    /// Endpoint-specific command shown in update instructions.
    pub update_install_command: String,
    /// Endpoint's normalized built-in keybindings, used only when a remote client selects server bindings.
    pub server_keybindings_toml: Option<String>,
    /// Whether the endpoint has a What's New entry, even if its body is unavailable.
    pub latest_release_notes_available: bool,
    /// Cached endpoint-owned notes used by the client-rendered overlay.
    pub release_notes: Option<ClientShellReleaseNotes>,
    pub focused_workspace_id: Option<String>,
    pub focused_tab_id: Option<String>,
    pub focused_pane_id: Option<String>,
    pub tab_bar_right: Vec<ClientShellTabStatusSegment>,
    pub tab_bar_right_separator: String,
    pub agent_view_label: Option<String>,
    pub agent_order: Vec<String>,
    pub workspaces: Vec<ClientShellWorkspace>,
    pub tabs: Vec<ClientShellTab>,
    pub panes: Vec<ClientShellPane>,
    pub agents: Vec<ClientShellAgent>,
    pub commands: Vec<ClientShellCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellProductAnnouncement {
    pub version: String,
    pub id: String,
    pub title: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellReleaseNotes {
    pub version: String,
    pub body: String,
    pub preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientShellCommandAction {
    Shell,
    Pane,
    PluginAction,
    /// A future endpoint action kind that this client cannot execute.
    #[serde(other)]
    Unknown,
}

impl From<crate::config::CustomCommandAction> for ClientShellCommandAction {
    fn from(action: crate::config::CustomCommandAction) -> Self {
        match action {
            crate::config::CustomCommandAction::Shell => Self::Shell,
            crate::config::CustomCommandAction::Pane => Self::Pane,
            crate::config::CustomCommandAction::PluginAction => Self::PluginAction,
        }
    }
}

impl TryFrom<ClientShellCommandAction> for crate::config::CustomCommandAction {
    type Error = ();

    fn try_from(action: ClientShellCommandAction) -> Result<Self, Self::Error> {
        match action {
            ClientShellCommandAction::Shell => Ok(Self::Shell),
            ClientShellCommandAction::Pane => Ok(Self::Pane),
            ClientShellCommandAction::PluginAction => Ok(Self::PluginAction),
            ClientShellCommandAction::Unknown => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellCommand {
    pub command_id: String,
    pub binding_label: String,
    pub binding_labels: Vec<String>,
    pub action: ClientShellCommandAction,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellTabStatusSegment {
    pub text: String,
    pub accent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorkspace {
    pub workspace_id: String,
    pub active_tab_id: String,
    pub new_workspace_cwd: String,
    pub number: usize,
    pub label: String,
    pub custom_label: bool,
    pub branch: Option<String>,
    pub git_ahead_behind: Option<(usize, usize)>,
    pub tokens: Vec<(String, String)>,
    pub worktree: Option<ClientShellWorktree>,
    pub focused: bool,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: crate::api::schema::AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorktree {
    pub key: String,
    pub label: String,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellTab {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub custom_label: bool,
    pub zoomed: bool,
    pub focused: bool,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: crate::api::schema::AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellPane {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub focused: bool,
    pub right_click_passthrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellAgent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub name: Option<String>,
    pub display_agent: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    #[serde(deserialize_with = "deserialize_client_shell_agent_status")]
    pub agent_status: crate::api::schema::AgentStatus,
    pub state_change_seq: u64,
    pub state_labels: Vec<(String, String)>,
    pub tokens: Vec<(String, String)>,
    pub focused: bool,
}

/// Origin-relative geometry for one pane in a rendered pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePane {
    pub pane_id: String,
    pub content_revision: u64,
    pub rect: SurfaceRect,
    pub inner_rect: SurfaceRect,
    pub scrollbar_rect: Option<SurfaceRect>,
    pub scroll: Option<PaneSurfaceScrollMetrics>,
    pub focused: bool,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub alternate_screen_active: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceScrollMetrics {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

/// One draggable BSP split handle relative to a pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceSplit {
    pub direction: PaneSurfaceSplitDirection,
    pub pos: u16,
    pub area: SurfaceRect,
    pub hit_rect: SurfaceRect,
    pub path: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneSurfaceSplitDirection {
    Horizontal,
    Vertical,
}

/// Wire-safe rectangle relative to a pane surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl From<ratatui::layout::Rect> for SurfaceRect {
    fn from(rect: ratatui::layout::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsTarget {
    Pane { pane_id: String },
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsSource {
    Terminal {
        target: SurfaceGraphicsTarget,
        image_id: u32,
    },
    PaneLayer {
        pane_id: String,
        layer_id: String,
    },
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsFormat {
    Rgb,
    Rgba,
    Png,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAssetKey {
    pub source: SurfaceGraphicsSource,
    pub image_width: u32,
    pub image_height: u32,
    pub format: SurfaceGraphicsFormat,
    pub data_len: u64,
    pub data_fingerprint: u64,
}

/// Image bytes newly needed by this connection's complete desired scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAsset {
    pub key: SurfaceGraphicsAssetKey,
    #[serde(
        serialize_with = "serialize_graphics_bytes",
        deserialize_with = "deserialize_graphics_bytes"
    )]
    pub data: Vec<u8>,
}

fn serialize_graphics_bytes<S: serde::Serializer>(
    data: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    // Bincode's byte slice has the same length+bytes layout as Vec<u8>,
    // but avoids per-byte serialization. Keep human-readable codecs unchanged.
    if serializer.is_human_readable() {
        data.serialize(serializer)
    } else {
        serializer.serialize_bytes(data)
    }
}

fn deserialize_graphics_bytes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    if deserializer.is_human_readable() {
        return Vec::<u8>::deserialize(deserializer);
    }

    struct BytesVisitor;
    impl<'de> serde::de::Visitor<'de> for BytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("image bytes")
        }

        fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
            Ok(bytes.to_vec())
        }

        fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
            Ok(bytes)
        }
    }

    // The framed slice decoder checks the length against available input before
    // handing bytes to the visitor, so a forged length cannot cause an allocation.
    deserializer.deserialize_bytes(BytesVisitor)
}

/// One already-clipped desired placement relative to its target surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsPlacement {
    pub asset: SurfaceGraphicsAssetKey,
    pub logical_placement_id: u32,
    pub x: u16,
    pub y: u16,
    pub cols: u32,
    pub rows: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z: i32,
    pub scrollback_offset: u32,
}

/// Complete desired placements plus only the image bytes not already sent for
/// the current live scene on this connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsScene {
    pub assets: Vec<SurfaceGraphicsAsset>,
    pub placements: Vec<SurfaceGraphicsPlacement>,
    /// Direct-uploaded assets that remain live for this client even while their
    /// pane is outside the selected scene.
    pub retained_assets: Vec<SurfaceGraphicsAssetKey>,
}

/// One server-rendered active-tab surface without sidebar, tab bar, or overlays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceFrame {
    /// Endpoint process identity that produced this surface.
    pub boot_id: String,
    /// Projection revision whose focused IDs and topology produced this surface.
    pub projection_revision: u64,
    /// Monotonic revision for full surfaces and incremental patches on one connection.
    pub surface_revision: u64,
    pub frame: FrameData,
    pub panes: Vec<PaneSurfacePane>,
    pub splits: Vec<PaneSurfaceSplit>,
    pub graphics: SurfaceGraphicsScene,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatchRow {
    /// Origin-relative surface column where this changed cell span starts.
    pub x: u16,
    /// Origin-relative surface row.
    pub y: u16,
    pub cells: Vec<CellData>,
}

/// Incremental terminal-cell update against one committed complete pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatch {
    pub boot_id: String,
    pub projection_revision: u64,
    pub base_surface_revision: u64,
    pub surface_revision: u64,
    pub rows: Vec<PaneSurfacePatchRow>,
    /// Updated metadata for panes whose terminal content changed.
    pub panes: Vec<PaneSurfacePane>,
    /// Final cursor relative to the pane surface.
    pub cursor: Option<CursorState>,
}

/// Terminal ANSI bytes encoded by the server for network-efficient clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFrame {
    /// Monotonic per-client frame sequence.
    pub seq: u64,
    /// Frame width in columns.
    pub width: u16,
    /// Frame height in rows.
    pub height: u16,
    /// Whether bytes contain a full redraw rather than an incremental diff.
    pub full: bool,
    /// Terminal escape bytes ready to write directly to stdout.
    pub bytes: Vec<u8>,
}

/// Notification kind forwarded from server to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifyKind {
    /// Play a sound (bell/agent-done, etc.).
    Sound,
    /// Display a toast message through the outer terminal.
    Toast,
    /// Display a toast message through the host OS notification service.
    SystemToast,
}

/// A client-rendered notification category. The server reports the semantic
/// event; each connected shell client chooses how (or whether) to present it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticNotificationKind {
    NeedsAttention,
    Finished,
    UpdateInstalled,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticNotificationSound {
    Done,
    Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticNotification {
    pub kind: SemanticNotificationKind,
    pub title: String,
    pub body: Option<String>,
    pub sound: Option<SemanticNotificationSound>,
    pub agent: Option<String>,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
    pub position: Option<crate::config::ToastHerdrPosition>,
}

/// Approximate retained cost of one queued in-process client message.
///
/// In-process transport carries typed values, so this only bounds memory rather
/// than measuring an encoded envelope.
pub fn queued_message_cost(message: &ClientMessage) -> usize {
    match message {
        ClientMessage::Input { data } => data.len(),
        ClientMessage::ClipboardImage { data, .. } => data.len(),
        ClientMessage::ClientShellPaneInput { pane_id, events } => {
            let payload: usize = events
                .iter()
                .map(|event| match event {
                    ClientPaneInputEvent::Paste(text) | ClientPaneInputEvent::TextCommit(text) => {
                        text.len()
                    }
                    ClientPaneInputEvent::Key { generated_text, .. } => generated_text
                        .as_ref()
                        .map_or(std::mem::size_of::<ClientPaneInputEvent>(), String::len),
                    ClientPaneInputEvent::Mouse { .. } => {
                        std::mem::size_of::<ClientPaneInputEvent>()
                    }
                })
                .sum();
            payload.saturating_add(pane_id.len())
        }
        ClientMessage::ClientShellEndpointRequest { request, .. } => {
            std::mem::size_of_val(request.as_ref())
        }
        _ => std::mem::size_of::<ClientMessage>(),
    }
}

/// Failure payload of one endpoint operation, carried typed instead of as an
/// `ErrorResponse` JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointResponseError {
    pub code: Option<String>,
    pub message: String,
}

/// Messages sent from the Core adapter to the client shell as typed values.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// Terminal bytes to write directly for a terminal-ANSI client.
    Terminal(TerminalFrame),

    /// Client-local Kitty graphics bytes to write directly to the host terminal.
    Graphics {
        /// Raw Kitty graphics protocol bytes.
        bytes: Vec<u8>,
    },

    /// Server is shutting down. Clients should exit gracefully.
    ServerShutdown {
        /// Optional reason for the shutdown.
        reason: Option<String>,
    },

    /// A notification event (sound/toast) to be rendered locally by the client.
    Notify {
        /// What kind of notification.
        kind: NotifyKind,
        /// Human-readable title or sound label.
        message: String,
        /// Optional human-readable notification body.
        body: Option<String>,
    },

    /// OSC 52 clipboard data forwarded from a PTY through the server.
    Clipboard {
        /// Base64-encoded clipboard data.
        data: String,
    },

    /// Set the foreground client's outer terminal window title.
    WindowTitle {
        /// Sanitized title to write with OSC 0. `None` restores Herdr's default title.
        title: Option<String>,
    },

    /// Client-local runtime config changed on disk; refresh it without reconnecting.
    ReloadSoundConfig,

    /// Whether the client should currently capture host mouse input.
    MouseCapture {
        /// True when Herdr mouse UI is enabled or the focused pane app requests mouse reporting.
        enabled: bool,
        /// True only while the focused pane requests DEC SGR pixel mode 1016.
        sgr_pixels: bool,
    },

    /// Ring the foreground client's outer terminal for pane-originated BEL characters.
    TerminalBell {
        /// Number of BEL characters parsed from one PTY read.
        count: u16,
    },

    /// One validated Herdr-owned Kitty regular-file RGBA transmission.
    GraphicsFile {
        path: String,
        expected_len: u64,
        image_id: u32,
        transfer_id: u64,
        leading: Vec<u8>,
        control: String,
        /// ClientShell upload identity. `None` targets a direct terminal client.
        surface_asset: Option<SurfaceGraphicsAssetKey>,
    },

    /// Suppress a direct command that expired before terminal delivery.
    GraphicsTransmissionRetired { transfer_id: u64, image_id: u32 },

    /// Initial metadata for a client-owned shell.
    ClientShellSnapshot(Box<ClientShellSnapshot>),

    /// Active-tab pane content rendered at a client-requested origin-relative size.
    PaneSurface(PaneSurfaceFrame),

    /// Ephemeral semantic notification delivered over the private control lane.
    /// It is sent only to currently connected client-rendered shells.
    SemanticNotification(SemanticNotification),

    /// Immediate endpoint error that the client-rendered shell must show regardless of notification policy.
    ClientShellError { message: String },

    /// Exact Kitty keyboard flags requested by a directly attached terminal.
    /// Zero restores the host terminal's previous keyboard mode.
    DirectTerminalKeyboardProtocol {
        flags: u16,
        modify_other_keys_level: u8,
    },

    /// Whether the focused pane or popup needs the shell host to report every key.
    ClientShellKeyboardReportAll { enabled: bool },

    /// API methods this connection's Core actually implements. Re-sent whenever a
    /// replacement connection changes the advertised set.
    ClientShellEndpointMethods { methods: Vec<String> },

    /// Final result of one endpoint operation.
    ClientShellEndpointResponse {
        boot_id: String,
        request_id: String,
        result: Result<Box<crate::api::schema::ResponseResult>, EndpointResponseError>,
    },

    /// Incremental terminal-cell update for a previously committed pane surface.
    PaneSurfacePatch(PaneSurfacePatch),
}

// ---------------------------------------------------------------------------
// Color / Modifier conversion helpers
// ---------------------------------------------------------------------------

/// Converts a ratatui `Color` to a packed u32 for wire transport.
///
/// Encoding:
/// - Named colors (Reset, Black, …, White) → `0x00_00_00_XX` where XX is 0..=16
/// - Indexed palette → `0x01_00_00_XX` where XX is the palette index
/// - RGB → `0x02_RR_GG_BB` with components in the lower 3 bytes
pub(crate) fn color_to_u32(color: ratatui::style::Color) -> u32 {
    match color {
        ratatui::style::Color::Reset => 0x00_00_00_00,
        ratatui::style::Color::Black => 0x00_00_00_01,
        ratatui::style::Color::Red => 0x00_00_00_02,
        ratatui::style::Color::Green => 0x00_00_00_03,
        ratatui::style::Color::Yellow => 0x00_00_00_04,
        ratatui::style::Color::Blue => 0x00_00_00_05,
        ratatui::style::Color::Magenta => 0x00_00_00_06,
        ratatui::style::Color::Cyan => 0x00_00_00_07,
        ratatui::style::Color::Gray => 0x00_00_00_08,
        ratatui::style::Color::DarkGray => 0x00_00_00_09,
        ratatui::style::Color::LightRed => 0x00_00_00_0A,
        ratatui::style::Color::LightGreen => 0x00_00_00_0B,
        ratatui::style::Color::LightYellow => 0x00_00_00_0C,
        ratatui::style::Color::LightBlue => 0x00_00_00_0D,
        ratatui::style::Color::LightMagenta => 0x00_00_00_0E,
        ratatui::style::Color::LightCyan => 0x00_00_00_0F,
        ratatui::style::Color::White => 0x00_00_00_10,
        ratatui::style::Color::Indexed(i) => 0x01_00_00_00 | (i as u32),
        ratatui::style::Color::Rgb(r, g, b) => {
            0x02_00_00_00 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        }
    }
}

/// Converts a packed u32 back to a ratatui `Color`.
fn u32_to_color(val: u32) -> ratatui::style::Color {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => ratatui::style::Color::Reset,
            0x01 => ratatui::style::Color::Black,
            0x02 => ratatui::style::Color::Red,
            0x03 => ratatui::style::Color::Green,
            0x04 => ratatui::style::Color::Yellow,
            0x05 => ratatui::style::Color::Blue,
            0x06 => ratatui::style::Color::Magenta,
            0x07 => ratatui::style::Color::Cyan,
            0x08 => ratatui::style::Color::Gray,
            0x09 => ratatui::style::Color::DarkGray,
            0x0A => ratatui::style::Color::LightRed,
            0x0B => ratatui::style::Color::LightGreen,
            0x0C => ratatui::style::Color::LightYellow,
            0x0D => ratatui::style::Color::LightBlue,
            0x0E => ratatui::style::Color::LightMagenta,
            0x0F => ratatui::style::Color::LightCyan,
            0x10 => ratatui::style::Color::White,
            _ => ratatui::style::Color::Reset, // unknown named → Reset
        },
        0x01 => ratatui::style::Color::Indexed((val & 0xFF) as u8),
        0x02 => {
            let r = ((val >> 16) & 0xFF) as u8;
            let g = ((val >> 8) & 0xFF) as u8;
            let b = (val & 0xFF) as u8;
            ratatui::style::Color::Rgb(r, g, b)
        }
        _ => ratatui::style::Color::Reset, // unknown tag → Reset
    }
}

const UNDERLINE_STYLE_SHIFT: u16 = 12;
const UNDERLINE_STYLE_MASK: u16 = 0xF000;

/// Converts a ratatui `Modifier` bitmask to a u16 for wire transport.
pub(crate) fn modifier_to_u16(modifier: ratatui::style::Modifier) -> u16 {
    modifier.bits()
}

pub(crate) fn underline_style_from_modifier(modifier: u16) -> u8 {
    ((modifier & UNDERLINE_STYLE_MASK) >> UNDERLINE_STYLE_SHIFT) as u8
}

pub(crate) fn modifier_with_underline_style(
    modifier: ratatui::style::Modifier,
    underline_style: u8,
) -> ratatui::style::Modifier {
    let bits = modifier.bits() | ((u16::from(underline_style) & 0x0F) << UNDERLINE_STYLE_SHIFT);
    ratatui::style::Modifier::from_bits_retain(bits)
}

/// Converts a u16 back to a ratatui `Modifier`.
fn u16_to_modifier(val: u16) -> ratatui::style::Modifier {
    ratatui::style::Modifier::from_bits_truncate(val & !UNDERLINE_STYLE_MASK)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::style::{Color, Modifier};
    #[test]
    fn client_shell_pane_input_reconstructs_windows_dead_key_without_native_source() {
        let event = ClientPaneInputEvent::Key {
            code: ClientKeyCode::Char('6'),
            modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            kind: ClientKeyKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: true,
            physical_key_id: Some(0x07),
            windows_record: Some(crate::input::WindowsKeyRecord {
                key_down: true,
                repeat_count: 1,
                virtual_key_code: 0x36,
                virtual_scan_code: 0x07,
                unicode: 0,
                control_key_state: 0x0030,
            }),
        };
        let crate::raw_input::RawInputEvent::Key(key) =
            event.to_raw_input_event_with_windows_source(false)
        else {
            panic!("pane dead key should remain a key");
        };
        assert!(key.is_windows_dead_key());
        assert_eq!(key.windows_record(), None);
        assert!(crate::pane::test_encode_key_for_app(b"\x1b[>1u", key).is_empty());
    }

    #[tokio::test]
    async fn client_shell_remote_altgr_dead_key_emits_only_composed_text() {
        // Spanish ISO AltGr+4, then Space, captured in #3948:
        // https://github.com/herdrdev/herdr/issues/3948#issuecomment-5633222390
        let records = [
            ('4', ClientKeyKind::Press, 52, 5, 0, 9),
            ('4', ClientKeyKind::Release, 52, 5, 0, 9),
            ('~', ClientKeyKind::Press, 32, 57, 126, 0),
            (' ', ClientKeyKind::Release, 32, 57, 32, 0),
        ];
        let runtime = crate::pane::testing::Screen::new(80, 24);
        let mut output = Vec::new();
        for (ch, kind, virtual_key_code, virtual_scan_code, unicode, control_key_state) in records {
            let event = ClientInputEvent::Key {
                code: ClientKeyCode::Char(ch),
                modifiers: 0,
                kind,
                repeat_count: 1,
                generated_text: None,
                source: ClientKeySource::WindowsConsole {
                    record: crate::input::WindowsKeyRecord {
                        key_down: kind == ClientKeyKind::Press,
                        repeat_count: 1,
                        virtual_key_code,
                        virtual_scan_code,
                        unicode,
                        control_key_state,
                    },
                },
            };
            let crate::raw_input::RawInputEvent::Key(key) = event.to_raw_input_event() else {
                panic!("captured input should remain a key");
            };
            let pane_event = ClientPaneInputEvent::from_terminal_key(key).expect("pane key");
            let crate::raw_input::RawInputEvent::Key(key) =
                pane_event.to_raw_input_event_with_windows_source(false)
            else {
                panic!("remote input should remain a key");
            };
            let bytes = runtime.encode_terminal_key(key);
            let expected: &[u8] = if ch == '~' { b"~" } else { b"" };
            assert_eq!(bytes, expected, "captured {ch:?} {kind:?}");
            output.extend(bytes);
        }
        assert_eq!(output, b"~", "dead key must not insert its base character");
    }

    #[test]
    fn client_shell_key_roundtrip_preserves_physical_generated_text_encoding() {
        let key = crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Char('/'),
            crossterm::event::KeyModifiers::SHIFT,
        )
        .with_generated_text(Some("/".into()))
        .with_windows_record(crate::input::WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x37,
            virtual_scan_code: 0x08,
            unicode: '/' as u16,
            control_key_state: 0,
        });
        let event = ClientPaneInputEvent::from_terminal_key(key).expect("semantic pane key");
        assert!(matches!(
            event,
            ClientPaneInputEvent::Key {
                tracks_release: true,
                ..
            }
        ));
        let crate::raw_input::RawInputEvent::Key(roundtripped) = event.to_raw_input_event() else {
            panic!("pane key should remain a key");
        };

        assert!(roundtripped.has_physical_identity());
        let encoded = crate::pane::test_encode_key_for_app(b"\x1b[>8u", roundtripped);
        assert_ne!(encoded, b"/");
        assert!(encoded.starts_with(b"\x1b["));
    }

    // ---- FrameData ↔ ratatui Buffer conversion ----

    #[test]
    fn frame_data_roundtrip_through_ratatui_buffer() {
        let area = ratatui::layout::Rect::new(0, 0, 5, 3);
        let mut buffer = ratatui::buffer::Buffer::filled(area, ratatui::buffer::Cell::new(" "));

        // Write some styled content.
        buffer.cell_mut((0, 0)).unwrap().set_symbol("H");
        buffer.cell_mut((0, 0)).unwrap().fg = Color::Red;
        buffer.cell_mut((0, 0)).unwrap().modifier = Modifier::BOLD;

        buffer.cell_mut((1, 0)).unwrap().set_symbol("i");
        buffer.cell_mut((1, 0)).unwrap().fg = Color::Green;
        buffer.cell_mut((1, 0)).unwrap().modifier = Modifier::ITALIC;

        buffer.cell_mut((2, 0)).unwrap().set_symbol("!");
        buffer.cell_mut((2, 0)).unwrap().fg = Color::Rgb(255, 128, 0);
        buffer.cell_mut((2, 0)).unwrap().bg = Color::Indexed(220);

        let cursor = CursorState {
            x: 1,
            y: 0,
            visible: true,
            shape: 0,
        };
        let frame = FrameData::from_ratatui_buffer(&buffer, Some(cursor.clone()));

        // Verify frame dimensions.
        assert_eq!(frame.width, 5);
        assert_eq!(frame.height, 3);
        assert_eq!(frame.cells.len(), 15);
        assert_eq!(frame.cursor, Some(cursor));

        // Verify specific cells survived the conversion.
        assert_eq!(frame.cells[0].symbol, "H");
        assert_eq!(frame.cells[0].fg, color_to_u32(Color::Red));
        assert_eq!(frame.cells[0].modifier, Modifier::BOLD.bits());

        assert_eq!(frame.cells[1].symbol, "i");
        assert_eq!(frame.cells[1].fg, color_to_u32(Color::Green));
        assert_eq!(frame.cells[1].modifier, Modifier::ITALIC.bits());

        assert_eq!(frame.cells[2].symbol, "!");
        assert_eq!(frame.cells[2].fg, color_to_u32(Color::Rgb(255, 128, 0)));
        assert_eq!(frame.cells[2].bg, color_to_u32(Color::Indexed(220)));

        let with_links = FrameData::from_ratatui_buffer_with_hyperlinks(
            &buffer,
            None,
            &[((1, 0), "i".to_owned(), "https://example.com".to_owned())],
        );
        assert_eq!(with_links.cells[1].hyperlink, Some(0));
        assert_eq!(
            with_links.hyperlinks,
            vec!["https://example.com".to_owned()]
        );

        // Convert back to ratatui buffer and compare.
        let restored = frame.to_ratatui_buffer().expect("should reconstruct");
        assert_eq!(restored.area, area);
        assert_eq!(restored.cell((0, 0)).unwrap().symbol(), "H");
        assert_eq!(restored.cell((0, 0)).unwrap().fg, Color::Red);
        assert_eq!(restored.cell((0, 0)).unwrap().modifier, Modifier::BOLD);
        assert_eq!(restored.cell((1, 0)).unwrap().symbol(), "i");
        assert_eq!(restored.cell((2, 0)).unwrap().symbol(), "!");
        assert_eq!(restored.cell((2, 0)).unwrap().fg, Color::Rgb(255, 128, 0));
    }

    #[test]
    fn frame_data_rejects_mismatched_cell_count() {
        let frame = FrameData {
            cells: vec![
                CellData {
                    symbol: "X".into(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                };
                5
            ], // 5 cells but 3×2 = 6 expected
            width: 3,
            height: 2,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        };
        assert!(frame.to_ratatui_buffer().is_none());
    }

    // ---- Color conversion coverage ----

    #[test]
    fn color_roundtrip_all_named_colors() {
        let named = [
            Color::Reset,
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ];
        for c in named {
            assert_eq!(
                u32_to_color(color_to_u32(c)),
                c,
                "roundtrip failed for {c:?}"
            );
        }
    }

    #[test]
    fn color_roundtrip_indexed() {
        for i in 0..=255u8 {
            let c = Color::Indexed(i);
            assert_eq!(
                u32_to_color(color_to_u32(c)),
                c,
                "roundtrip failed for Indexed({i})"
            );
        }
    }

    #[test]
    fn color_roundtrip_rgb() {
        let c = Color::Rgb(0xAB, 0xCD, 0xEF);
        assert_eq!(u32_to_color(color_to_u32(c)), c);

        let c = Color::Rgb(0, 0, 0);
        assert_eq!(u32_to_color(color_to_u32(c)), c);

        let c = Color::Rgb(255, 255, 255);
        assert_eq!(u32_to_color(color_to_u32(c)), c);
    }

    // ---- Modifier conversion ----

    #[test]
    fn modifier_roundtrip() {
        let all_mods = [
            Modifier::BOLD,
            Modifier::ITALIC,
            Modifier::REVERSED,
            Modifier::UNDERLINED,
            Modifier::DIM,
            Modifier::SLOW_BLINK,
            Modifier::CROSSED_OUT,
            Modifier::BOLD | Modifier::ITALIC,
            Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED,
            Modifier::empty(),
        ];
        for m in all_mods {
            assert_eq!(
                u16_to_modifier(modifier_to_u16(m)),
                m,
                "roundtrip failed for {m:?}"
            );
        }
    }
}
