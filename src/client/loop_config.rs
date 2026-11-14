use super::*;

pub(super) struct ClientLoopConfig {
    pub(super) host_cursor: crate::config::HostCursorModeConfig,
    pub(super) mouse_capture_active: bool,
    pub(super) host_escape_disambiguation_active: bool,
    pub(super) host_sgr_pixel_mouse: Option<bool>,
    pub(super) initial_host_input: Vec<u8>,
    pub(super) shell_config: shell::ClientShellConfig,
}
