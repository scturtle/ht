use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "aarch64-apple-ios" => "aarch64-ios",
        "aarch64-apple-ios-sim" => "aarch64-ios-simulator",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("unsupported target for libghostty-vt build: {other}"),
    }
}

fn env_bool(name: &str) -> Option<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            other => panic!("invalid boolean value for {name}: {other}"),
        },
        Err(env::VarError::NotPresent) => None,
        Err(err) => panic!("failed to read {name}: {err}"),
    }
}

/// Copy the pristine submodule into `OUT_DIR` and apply the local patches there.
///
/// The submodule is a plain upstream checkout, so patching in place would dirty
/// it and make `git submodule update` destructive. Staging also keeps builds of
/// different targets from sharing one patched tree.
fn stage_patched_source(source_dir: &Path, patch_dir: &Path, out_dir: &Path) -> PathBuf {
    let stage = out_dir.join("libghostty-vt");
    let patches = list_patches(patch_dir);
    let stamp = stage.join(".staged");
    let wanted = stage_fingerprint(source_dir, &patches);
    if stamp.is_file() && fs::read_to_string(&stamp).ok().as_deref() == Some(wanted.as_str()) {
        return stage;
    }
    if stage.exists() {
        fs::remove_dir_all(&stage).expect("failed to clear staged libghostty-vt source");
    }
    fs::create_dir_all(&stage).expect("failed to create staged libghostty-vt source");
    for entry in [
        "build.zig",
        "build.zig.zon",
        "LICENSE",
        "include",
        "src",
        "pkg",
        "dist",
    ] {
        let from = source_dir.join(entry);
        if !from.exists() {
            continue;
        }
        let to = stage.join(entry);
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            fs::copy(&from, &to).expect("failed to copy libghostty-vt file");
        }
    }
    // The tag ships neither version file; the dist tarball normally adds them and
    // build.zig reads LIB_VERSION for the library version.
    fs::write(stage.join("VERSION"), "1.0.0\n").expect("failed to write staged VERSION");
    fs::write(stage.join("LIB_VERSION"), "1.0.0\n").expect("failed to write staged LIB_VERSION");

    for (path, name) in &patches {
        let patch_bytes = fs::read(path).expect("failed to read libghostty-vt patch");
        let mut child = Command::new("patch")
            .arg("-p1")
            .arg("--forward")
            .arg("--fuzz=0")
            .arg("--no-backup-if-mismatch")
            .current_dir(&stage)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("failed to run patch for {name}: {err}"));
        {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("patch stdin")
                .write_all(&patch_bytes)
                .expect("failed to feed libghostty-vt patch");
        }
        let status = child.wait().expect("failed to wait for patch");
        assert!(
            status.success(),
            "libghostty-vt patch {name} did not apply cleanly; regenerate it against the \
             pinned submodule commit"
        );
    }
    fs::write(&stamp, wanted).expect("failed to write staged source fingerprint");
    stage
}

fn list_patches(patch_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut patches = fs::read_dir(patch_dir)
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "patch"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    patches.sort();
    patches
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (path, name)
        })
        .collect()
}

/// Fingerprint the submodule commit plus every patch, so a checkout that moved
/// either side restages instead of reusing a stale patched tree.
fn stage_fingerprint(source_dir: &Path, patches: &[(PathBuf, String)]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    for patch in [
        source_dir.join("build.zig"),
        source_dir.join("build.zig.zon"),
    ] {
        if let Ok(bytes) = fs::read(&patch) {
            hasher.update(&bytes);
        }
    }
    for (path, _) in patches {
        if let Ok(bytes) = fs::read(path) {
            hasher.update(&bytes);
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("failed to create staged directory");
    for entry in fs::read_dir(from).expect("failed to read source directory") {
        let entry = entry.expect("failed to read directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("failed to copy staged file");
        }
    }
}

fn main() {
    // The vendored source stays at the repository root, shared with its maintenance scripts.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_dir = manifest_dir.join("../..");
    let source_dir = repo_dir.join("3rdparty").join("libghostty-vt");
    let patch_dir = repo_dir
        .join("vendor")
        .join("patches")
        .join("libghostty-vt");

    println!("cargo:rerun-if-changed=build.rs");
    for path in [
        repo_dir.join("vendor").join("libghostty-vt.vendor.json"),
        source_dir.join("build.zig"),
        source_dir.join("build.zig.zon"),
        source_dir.join("include"),
        source_dir.join("pkg"),
        source_dir.join("src"),
        source_dir.join("dist").join("cmake"),
        patch_dir.clone(),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_WINDOWS_LIBC");

    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let simd = env_bool("LIBGHOSTTY_VT_SIMD").unwrap_or(true);
    let target = env::var("TARGET").expect("TARGET");
    let zig_target = zig_target(&target);
    // The pinned tag is vt-1.0.0; the staged tree carries the same value in
    // VERSION and LIB_VERSION, and zig records this one in build_info.
    let version_string = "1.0.0".to_string();

    // Install into this build's OUT_DIR so concurrent builds for different targets
    // from one checkout never link each other's archive.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    // The upstream submodule stays pristine: every build compiles a staged copy
    // with the local patches applied.
    let vendored_dir = stage_patched_source(&source_dir, &patch_dir, &out_dir);
    let zig_prefix = out_dir.join("zig-out");

    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("--prefix")
        .arg(&zig_prefix)
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version_string}"))
        .arg("-Demit-xcframework=false");
    if target.ends_with("windows-msvc") {
        if let Some(libc_file) = env::var_os("LIBGHOSTTY_VT_WINDOWS_LIBC") {
            println!(
                "cargo:rerun-if-changed={}",
                PathBuf::from(&libc_file).display()
            );
            command.arg("--libc").arg(libc_file);
        }
    }
    if let Ok(system_dir) = env::var("LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }

    let status = command
        .current_dir(&vendored_dir)
        .status()
        .unwrap_or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                panic!(
                    "zig executable not found (looked for {zig:?}; set the ZIG \
                     environment variable to point at the zig binary). Building \
                     the vendored libghostty-vt requires Zig 0.17.0: install it from \
                     https://ziglang.org/download/, then retry the build"
                );
            }
            panic!("failed to execute zig build for vendored libghostty-vt: {err}");
        });
    assert!(
        status.success(),
        "zig build for vendored libghostty-vt failed: {status}. \
         Building Herdr requires Zig 0.17.0; check `zig version` \
         or set ZIG to the path of a Zig 0.17.0 binary, then retry"
    );

    let mut lib_dir = zig_prefix.join("lib");
    if target.contains("-apple-") {
        // Apple's linker prefers the sibling dylib for `-l`, so search a directory
        // holding only the static archive.
        let static_dir = out_dir.join("lib");
        fs::create_dir_all(&static_dir).expect("failed to create libghostty-vt link directory");
        fs::copy(
            lib_dir.join("libghostty-vt.a"),
            static_dir.join("libghostty-vt.a"),
        )
        .expect("failed to copy libghostty-vt static archive");
        lib_dir = static_dir;
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-lib=static=ghostty-vt-static");
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}
