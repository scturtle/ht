//! Presentation-only terminal oracle for host-input/codec conformance tests.
//! No PTY, process, server, or agent detection is involved.
use super::{GhosttyPaneTerminal, PaneTerminal};

pub(crate) struct Screen {
    pub(crate) terminal: PaneTerminal,
}
impl Screen {
    pub(crate) fn new(cols: u16, rows: u16) -> Self {
        let (_tx, _) = tokio::sync::mpsc::channel::<bytes::Bytes>(1);
        Self {
            terminal: PaneTerminal::new(
                GhosttyPaneTerminal::new(crate::ghostty::Terminal::new(cols, rows, 4096).unwrap())
                    .unwrap(),
            ),
        }
    }
    pub(crate) fn test_process_pty_bytes(&self, bytes: &[u8]) {
        let (_tx, _) = tokio::sync::mpsc::channel::<bytes::Bytes>(1);
        self.terminal
            .process_pty_bytes(crate::layout::PaneId::from_raw(1), bytes);
    }
    pub(crate) fn keyboard_protocol(&self) -> crate::input::KeyboardProtocol {
        self.terminal
            .keyboard_protocol(crate::input::KeyboardProtocol::Legacy)
    }
    pub(crate) fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        self.terminal.encode_terminal_key(key)
    }
}
