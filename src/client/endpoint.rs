//! The one Core connection of this client. Other ht clients are independent peers.
mod registry;
pub(crate) use registry::*;

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ClientEndpointId {
    #[default]
    Local,
}
impl ClientEndpointId {
    pub(crate) const fn is_local(&self) -> bool {
        true
    }
    pub(crate) fn storage_key(&self) -> String {
        "local".into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientEndpointStatus {
    /// No projection has arrived yet.
    Connecting,
    Online,
    #[cfg(test)]
    Reconnecting,
}
