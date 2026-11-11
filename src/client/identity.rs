use std::path::Path;

pub fn derive_label_from_cwd(cwd: &Path) -> String {
    fallback_label_from_cwd(cwd)
}

pub fn fallback_label_from_cwd(cwd: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home = Path::new(&home);
        if cwd == home {
            return "~".to_string();
        }
    }

    cwd.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cwd.display().to_string())
}
