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

/// The `hc` session this client drives. `hc` names its socket after it, so the
/// client reuses that convention instead of inventing a socket of its own.
static CORE_SESSION: OnceLock<String> = OnceLock::new();

pub fn set_core_session(name: String) {
    let _ = CORE_SESSION.set(name);
}

pub fn core_session() -> String {
    CORE_SESSION
        .get()
        .cloned()
        .unwrap_or_else(|| "default".to_string())
}

/// A client that picked its own session is expected to own its Core end to end.
pub fn core_session_is_private() -> bool {
    CORE_SESSION.get().is_some()
}

pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| crate::config::state_dir().join("core-client-v1"))
}
