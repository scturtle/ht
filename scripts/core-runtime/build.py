#!/usr/bin/env python3
"""Build pinned local Core sources; never installs or restarts an existing Core."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CACHE = ROOT / ".local/core-runtime"

# The pinned Core bundle vendors libghostty-vt from before 1.0 and still needs
# Zig 0.16. Herdr's own vendored tree is on 1.0, which needs 0.17. Keep the two
# toolchains apart instead of letting one ZIG export serve both.
CORE_ZIG_ENV = "HERDR_CORE_SOURCE_ZIG"
DEFAULT_CORE_ZIG = ROOT / ".local/toolchain/zig-py/ziglang/zig"


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def source_digest(source):
    result = hashlib.sha256()
    for parent, dirs, files in os.walk(source, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d not in {".git", "target", ".zig-cache", "zig-out", "zig-pkg"})
        paths = [Path(parent) / name for name in files]
        paths += [Path(parent) / name for name in dirs if (Path(parent) / name).is_symlink()]
        for path in sorted(paths):
            if path.name == ".core-source.json":
                continue
            result.update(path.relative_to(source).as_posix().encode() + b"\0")
            result.update(("link:" + os.readlink(path)).encode() if path.is_symlink() else digest(path).encode())
    return result.hexdigest()


def archive(name, spec):
    path = CACHE / "downloads" / (name + ".tar.gz")
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=name + "-", delete=False) as stream:
            pending = Path(stream.name)
            try:
                with urllib.request.urlopen(spec["url"], timeout=90) as response:
                    shutil.copyfileobj(response, stream)
                stream.close()
                if digest(pending) != spec["sha256"]:
                    raise RuntimeError(f"download digest mismatch: {name}")
                pending.replace(path)
            finally:
                pending.unlink(missing_ok=True)
    if digest(path) != spec["sha256"]:
        raise RuntimeError(f"cached archive changed: {path}; refusing to overwrite it")
    return path


def unpack(archive_path, destination):
    destination.mkdir()
    with tarfile.open(archive_path) as packed:
        packed.extractall(destination, filter="data")
    entries = list(destination.iterdir())
    if len(entries) != 1 or not entries[0].is_dir():
        raise RuntimeError("expected one source archive root")
    return entries[0]


def prepare():
    manifest_bytes = (HERE / "source.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    bundle = hashlib.sha256(manifest_bytes)
    patches = [HERE / "patches" / name for name in manifest["patches"]]
    for path in patches:
        bundle.update(path.read_bytes())
    bundle = bundle.hexdigest()
    builds = CACHE / "prepared"
    builds.mkdir(parents=True, exist_ok=True)
    source = builds / bundle[:20]
    stamp = source / ".core-source.json"
    if source.exists():
        saved = json.loads(stamp.read_text())
        if saved["bundle_sha256"] != bundle or saved["source_sha256"] != source_digest(source):
            raise RuntimeError(f"generated sources changed: {source}; refusing to discard edits")
        return source
    archives = {name: archive(name, manifest[name]) for name in ("core", "ghostty")}
    with tempfile.TemporaryDirectory(dir=builds, prefix="prepare-") as scratch:
        scratch = Path(scratch)
        core = unpack(archives["core"], scratch / "core-archive")
        ghostty = unpack(archives["ghostty"], scratch / "ghostty-archive")
        vendor = core / "vendor/ghostty"
        vendor.rmdir()  # A pinned submodule placeholder, not a populated checkout.
        ghostty.rename(vendor)
        for patch in patches:
            subprocess.run(
                ["patch", "--batch", "--forward", "--fuzz=0", "--no-backup-if-mismatch", "-p1", "-i", str(patch)],
                cwd=core,
                check=True,
                stdout=sys.stderr,
            )
        (core / ".core-source.json").write_text(json.dumps({"bundle_sha256": bundle, "source_sha256": source_digest(core), "core": manifest["core"]["commit"], "ghostty": manifest["ghostty"]["commit"]}, indent=2) + "\n")
        core.rename(source)
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "build", "test"))
    parser.add_argument("--debug", action="store_true", help="build an unoptimized Core (default: release)")
    args = parser.parse_args()
    source = prepare()
    if args.action == "prepare":
        print(source)
        return
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_"))}
    # Cargo build scripts do not necessarily fingerprint an external Ghostty
    # tree when the workspace path changes. Never reuse native artifacts from
    # another patched source bundle (even when crate versions are unchanged).
    target = CACHE / "artifacts" / source.name
    env["CARGO_TARGET_DIR"] = str(target)
    # The Core source bundle vendors its own libghostty snapshot, which still
    # needs Zig 0.16. Herdr's own vendored tree is on 1.0 and needs 0.17, so the
    # two builds must not share whichever ZIG the caller exported.
    core_zig = os.environ.get(CORE_ZIG_ENV)
    if core_zig:
        env["ZIG"] = core_zig
    elif DEFAULT_CORE_ZIG.exists():
        env["ZIG"] = str(DEFAULT_CORE_ZIG)
    else:
        env.pop("ZIG", None)
    command = ["cargo", "build" if args.action == "build" else "test", "--locked"]
    if args.action == "build":
        command += ["-p", "herdr-core"]
        if not args.debug:
            command += ["--release"]
    subprocess.run(command, cwd=source, env=env, check=True)
    if source_digest(source) != json.loads((source / ".core-source.json").read_text())["source_sha256"]:
        raise RuntimeError("build/test modified pinned sources; inspect before reuse")
    if args.action == "build":
        profile = "debug" if args.debug else "release"
        name = "herdr-core.exe" if os.name == "nt" else "herdr-core"
        output = CACHE / "target" / profile / name
        output.parent.mkdir(parents=True, exist_ok=True)
        fd, pending = tempfile.mkstemp(prefix=name + "-", dir=output.parent)
        os.close(fd)
        pending = Path(pending)
        try:
            shutil.copy2(target / profile / name, pending)
            pending.replace(output)  # Never expose a partial or failed build.
        finally:
            pending.unlink(missing_ok=True)
        print(output)


if __name__ == "__main__":
    main()
