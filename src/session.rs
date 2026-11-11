use std::path::PathBuf;
use std::sync::OnceLock;
#[cfg(test)]
pub const SESSION_ENV_VAR: &str = "HERDR_SESSION";

/// Set once at startup from `--core-home`. Kept out of the environment so a
/// client cannot silently retarget its Core because an enclosing process
/// exported a variable; only an explicit argument moves it.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| crate::config::state_dir().join("core-client-v1"))
}
