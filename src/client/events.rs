use std::io;

use super::host_terminal::HostGeometry;
use crate::actions::ActionResult;
use crate::protocol::{ClientShellSnapshot, EndpointResponseError, PaneSurfaceFrame};

/// Internal events for the client event loop.
pub(super) enum ClientLoopEvent {
    StdinInput(Vec<u8>),
    PixelMouse(Vec<u8>, HostGeometry),
    Resize(u16, u16, u32, u32, bool),
    TerminalUnavailable(io::Error),
    Endpoint(Box<EndpointEvent>),
    EndpointDetached,
    EndpointFailed,
    Timer,
}

/// Typed in-process output from the Core adapter, not part of the Core wire protocol.
#[derive(Debug)]
pub(super) enum EndpointEvent {
    Snapshot(Box<ClientShellSnapshot>),
    PaneSurface(PaneSurfaceFrame),
    Error {
        message: String,
    },
    Response {
        boot_id: String,
        request_id: String,
        result: Result<Box<ActionResult>, EndpointResponseError>,
    },
    Methods {
        methods: Vec<String>,
    },
    KeyboardReportAll {
        enabled: bool,
    },
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    TerminalBell {
        count: u16,
    },
    Clipboard {
        /// Base64-encoded OSC 52 clipboard data forwarded from a pane.
        data: String,
    },
}
