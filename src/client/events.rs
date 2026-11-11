use super::*;

/// Internal events for the client event loop.
pub(super) enum ClientLoopEvent {
    StdinInput(Vec<u8>),
    PixelMouse(Vec<u8>, crate::input::mouse::HostGeometry),
    DirectGraphicsResponse(direct_graphics::Response),
    Resize(u16, u16, u32, u32, bool),
    TerminalUnavailable(io::Error),
    ServerMessage {
        endpoint_id: endpoint::ClientEndpointId,
        generation: u64,
        message: Box<ServerMessage>,
    },
    ServerDisconnected {
        endpoint_id: endpoint::ClientEndpointId,
        generation: u64,
    },
    PaneDiagnosticsFinished {
        path: std::path::PathBuf,
        warning: Option<String>,
    },
    Timer,
}
