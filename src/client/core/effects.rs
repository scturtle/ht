//! Live terminal effects, separate from presentation invalidation and human input.
use crate::{pane::ProcessBytesResult, protocol::ServerMessage};
use base64::Engine as _;

pub(super) fn append(result: &ProcessBytesResult, messages: &mut Vec<ServerMessage>) {
    if result.terminal_bells > 0 {
        messages.push(ServerMessage::TerminalBell {
            count: result.terminal_bells,
        });
    }
    for bytes in &result.clipboard_writes {
        messages.push(ServerMessage::Clipboard {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::core::view::View, layout::PaneId};
    #[test]
    fn live_effects_and_query_replies_do_not_depend_on_presentation_visibility() {
        let mut view = View::new(80, 24, 1).unwrap();
        view.visible = false;
        let result = view.process_output(
            PaneId::from_raw(1),
            b"\x1b[?2026h\x07\x1b]52;c;Y2xpcGJvYXJk\x07\x1b[6n",
        );
        assert!(!result.request_render);
        assert_eq!(result.terminal_bells, 1);
        assert_eq!(result.clipboard_writes, vec![b"clipboard".to_vec()]);
        assert!(!result.terminal_responses.is_empty());
        let mut messages = vec![];
        append(&result, &mut messages);
        assert!(matches!(
            messages[0],
            ServerMessage::TerminalBell { count: 1 }
        ));
        assert!(matches!(&messages[1],ServerMessage::Clipboard{data} if data=="Y2xpcGJvYXJk"));
        view.visible = true;
        let done = view.process_output(PaneId::from_raw(1), b"\x1b[?2026l");
        assert!(done.request_render);
        assert_eq!(done.terminal_bells, 0);
        assert!(done.clipboard_writes.is_empty());
    }
}
