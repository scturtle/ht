use std::path::PathBuf;
#[cfg(test)]
pub const SESSION_ENV_VAR: &str = "HERDR_SESSION";

pub fn data_dir() -> PathBuf {
    std::env::var_os("HERDR_CORE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::state_dir().join("core-client-v1"))
}
