use super::*;

pub(super) struct ClientLoopConfig {
    pub(super) mouse_capture_active: bool,
    pub(super) host_escape_disambiguation_active: bool,
    pub(super) host_sgr_pixel_mouse: Option<bool>,
    pub(super) initial_host_input: Vec<u8>,
    pub(super) host_color_replies_expected: Arc<AtomicU64>,
    pub(super) shell_config: shell::ClientShellConfig,
}
