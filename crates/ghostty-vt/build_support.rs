//! Shared, directly tested source identity and staging-cache rules.
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub struct SourcePin {
    pub version: String,
    pub paths: Vec<PathBuf>,
    identity: Vec<u8>,
}

impl SourcePin {
    pub fn read(manifest: &Path, source: &Path) -> io::Result<Self> {
        let manifest_bytes = fs::read(manifest)?;
        let metadata: Value = serde_json::from_slice(&manifest_bytes)?;
        let entry = &metadata["libghostty_vt"];
        let stamp_bytes = fs::read(source.join(".libghostty-vt-source.json"))?;
        let stamp: Value = serde_json::from_slice(&stamp_bytes)?;
        for key in ["tag", "sha256", "paths"] {
            if entry[key].is_null() || entry[key] != stamp[key] {
                return Err(io::Error::other(format!(
                    "libghostty-vt source {key} does not match the vendor manifest; run just vendor-libghostty"
                )));
            }
        }
        let version = entry["tag"]
            .as_str()
            .and_then(|tag| tag.strip_prefix("vt-"))
            .filter(|v| {
                !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
            })
            .ok_or_else(|| io::Error::other("invalid libghostty-vt version tag"))?
            .to_owned();
        let paths = entry["paths"]
            .as_array()
            .ok_or_else(|| io::Error::other("invalid source paths"))?
            .iter()
            .map(|value| {
                let path = value
                    .as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| io::Error::other("invalid source path"))?;
                if path.as_os_str().is_empty()
                    || path
                        .components()
                        .any(|c| !matches!(c, Component::Normal(_)))
                {
                    return Err(io::Error::other(
                        "source paths must be relative and cannot traverse parents",
                    ));
                }
                Ok(path)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut identity = manifest_bytes;
        identity.extend_from_slice(&stamp_bytes);
        Ok(Self {
            version,
            paths,
            identity,
        })
    }

    pub fn fingerprint(&self, source: &Path, patches: &[(PathBuf, String)]) -> io::Result<String> {
        let mut hash = Sha256::new();
        // Changing staging rules invalidates old copies without a manual version bump.
        hash_part(&mut hash, include_bytes!("build.rs"));
        hash_part(&mut hash, include_bytes!("build_support.rs"));
        hash_part(&mut hash, &self.identity);
        for path in &self.paths {
            hash_part(&mut hash, path.as_os_str().as_encoded_bytes());
            hash_tree(&mut hash, &source.join(path))?;
        }
        for (path, name) in patches {
            hash_part(&mut hash, name.as_bytes());
            hash_part(&mut hash, &fs::read(path)?);
        }
        Ok(format!("{:x}", hash.finalize()))
    }
}

fn hash_part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn hash_tree(hash: &mut Sha256, path: &Path) -> io::Result<()> {
    if path.is_dir() {
        hash.update(b"directory");
        let mut entries = fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        hash.update((entries.len() as u64).to_le_bytes());
        for entry in entries {
            hash_part(
                hash,
                entry
                    .file_name()
                    .ok_or_else(|| io::Error::other("missing file name"))?
                    .as_encoded_bytes(),
            );
            hash_tree(hash, &entry)?;
        }
    } else {
        hash.update(b"file");
        hash_part(hash, &fs::read(path)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        root: PathBuf,
        manifest: PathBuf,
        source: PathBuf,
        entry: Value,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "herdr-stage-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let source = root.join("source");
            fs::create_dir_all(source.join("include")).unwrap();
            fs::write(source.join("build.zig"), "unchanged build").unwrap();
            fs::write(source.join("include/vt.h"), "old header").unwrap();
            let mut fixture = Self {
                manifest: root.join("vendor.json"),
                root,
                source,
                entry: json!({"tag":"vt-1.0.0", "sha256":"archive-a", "paths":["build.zig", "include"]}),
            };
            fixture.save();
            fixture
        }
        fn save(&mut self) {
            fs::write(
                &self.manifest,
                serde_json::to_vec(&json!({"libghostty_vt":self.entry})).unwrap(),
            )
            .unwrap();
            fs::write(
                self.source.join(".libghostty-vt-source.json"),
                serde_json::to_vec(&self.entry).unwrap(),
            )
            .unwrap();
        }
        fn pin(&self) -> SourcePin {
            SourcePin::read(&self.manifest, &self.source).unwrap()
        }
        fn fingerprint(&self) -> String {
            self.pin().fingerprint(&self.source, &[]).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn archive_identity_changes_invalidate_identical_build_files() {
        let mut fixture = Fixture::new();
        let previous = fixture.fingerprint();
        fixture.entry["sha256"] = json!("archive-b");
        fixture.save();
        assert_ne!(previous, fixture.fingerprint());
    }
    #[test]
    fn header_edits_invalidate_staging_without_a_new_archive() {
        let fixture = Fixture::new();
        let previous = fixture.fingerprint();
        fs::write(fixture.source.join("include/vt.h"), "new header").unwrap();
        assert_ne!(previous, fixture.fingerprint());
        assert_eq!(fixture.fingerprint(), fixture.fingerprint());
    }
    #[test]
    fn patch_names_contents_and_order_are_part_of_the_identity() {
        let fixture = Fixture::new();
        let patch = fixture.root.join("a.patch");
        fs::write(&patch, "patch a").unwrap();
        let fingerprint = |name: &str| {
            fixture
                .pin()
                .fingerprint(&fixture.source, &[(patch.clone(), name.into())])
                .unwrap()
        };
        let previous = fingerprint("a.patch");
        assert_ne!(previous, fingerprint("b.patch"));
        fs::write(&patch, "patch b").unwrap();
        assert_ne!(previous, fingerprint("a.patch"));
        let other = fixture.root.join("b.patch");
        fs::write(&other, "patch c").unwrap();
        let patches = [(patch, "a.patch".into()), (other, "b.patch".into())];
        let forward = fixture
            .pin()
            .fingerprint(&fixture.source, &patches)
            .unwrap();
        let reversed = fixture
            .pin()
            .fingerprint(&fixture.source, &[patches[1].clone(), patches[0].clone()])
            .unwrap();
        assert_ne!(forward, reversed);
    }
    #[test]
    fn version_comes_from_the_verified_tag_and_stale_sources_are_rejected() {
        let mut fixture = Fixture::new();
        assert_eq!(fixture.pin().version, "1.0.0");
        fixture.entry["tag"] = json!("vt-1.2.3");
        fs::write(
            &fixture.manifest,
            serde_json::to_vec(&json!({"libghostty_vt":fixture.entry})).unwrap(),
        )
        .unwrap();
        assert!(SourcePin::read(&fixture.manifest, &fixture.source).is_err());
        fixture.save();
        assert_eq!(fixture.pin().version, "1.2.3");
    }
    #[test]
    fn extraction_paths_cannot_escape_the_staging_directory() {
        let mut fixture = Fixture::new();
        for path in ["../outside", "/absolute", ""] {
            fixture.entry["paths"] = json!([path]);
            fixture.save();
            assert!(SourcePin::read(&fixture.manifest, &fixture.source).is_err());
        }
    }
}
