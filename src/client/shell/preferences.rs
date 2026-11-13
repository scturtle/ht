use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Retain the saved agent ordering for mobile and indexed agent shortcuts.
/// Retired sidebar geometry/group fields in older files are ignored by serde.
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct ClientChromePreferences {
    pub(super) agent_panel_sort: Option<crate::config::AgentPanelSortConfig>,
}

pub(super) fn path_for_local_endpoint(socket_path: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in socket_path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    crate::config::state_dir()
        .join("client-shell")
        .join(format!("local-{hash:016x}.json"))
}

pub(super) fn load(path: &Path) -> Option<ClientChromePreferences> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_paths_are_stable_and_distinct() {
        let first = path_for_local_endpoint(Path::new("/run/a.sock"));
        assert_eq!(first, path_for_local_endpoint(Path::new("/run/a.sock")));
        assert_ne!(first, path_for_local_endpoint(Path::new("/run/b.sock")));
        assert_ne!(first, path_for_local_endpoint(Path::new("/run/./a.sock")));
    }

    #[test]
    fn retired_sidebar_fields_do_not_discard_saved_agent_order() {
        let preferences: ClientChromePreferences = serde_json::from_str(
            r#"{"sidebar_width":32,"sidebar_collapsed":true,"collapsed_groups":["/repo"],"agent_panel_sort":"priority"}"#,
        ).expect("legacy preferences");
        assert_eq!(
            preferences.agent_panel_sort,
            Some(crate::config::AgentPanelSortConfig::Priority)
        );
        let legacy: ClientChromePreferences =
            serde_json::from_str(r#"{"collapsed_groups":["/repo"]}"#).expect("legacy fields");
        assert_eq!(legacy.agent_panel_sort, None);
    }
}
