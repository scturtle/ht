//! What a client learns before it can publish a frame.
//!
//! Core's connect path fills this in directly: ordered attach already tells the
//! client which endpoint methods exist and how frames are encoded, so there is
//! no separate negotiation reply to parse here.

use crate::protocol::RenderEncoding;

pub(super) struct HandshakeResult {
    pub(super) encoding: RenderEncoding,
    pub(super) endpoint_methods: Option<Vec<String>>,
    pub(super) endpoint_capabilities: Option<Vec<String>>,
}
