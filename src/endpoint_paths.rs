//! Historical local preference identity (not a live server endpoint).
pub(crate) fn client_socket_path() -> std::path::PathBuf {
    crate::config::config_dir().join("herdr-client.sock")
}
