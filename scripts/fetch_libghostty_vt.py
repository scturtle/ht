#!/usr/bin/env python3
"""Fetch the pinned libghostty-vt source tarball into the local cache.

libghostty-vt is a cargo build dependency, so the source has to exist before
`cargo build` runs. This script is that explicit step: it downloads the pinned
tag archive, verifies its digest, and extracts only the paths the build uses.

The tag URL is used instead of the commit URL because GitHub serves the tag
archive from a stable cache; two downloads of the same commit URL returned
different bytes (identical content, different compression stream), which would
make a digest pin useless.

Usage:
    python3 scripts/fetch_libghostty_vt.py           # fetch if missing
    python3 scripts/fetch_libghostty_vt.py --force    # re-extract
    python3 scripts/fetch_libghostty_vt.py --check    # verify only
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "vendor" / "libghostty-vt.vendor.json"
CACHE = ROOT / ".local" / "cache"
DOWNLOADS = CACHE / "downloads"
TARGET = ROOT / ".local" / "libghostty-vt"


def manifest() -> dict:
    return json.loads(MANIFEST.read_text())


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def download(entry: dict) -> Path:
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    archive = DOWNLOADS / f"libghostty-vt-{entry['tag']}.tar.gz"
    if archive.is_file() and digest(archive) == entry["sha256"]:
        return archive
    print(f"downloading {entry['url']}", file=sys.stderr)
    with tempfile.NamedTemporaryFile(dir=DOWNLOADS, delete=False) as scratch:
        partial = Path(scratch.name)
    try:
        subprocess.run(
            ["curl", "--fail", "--location", "--output", str(partial), entry["url"]],
            check=True,
        )
        actual = digest(partial)
        if actual != entry["sha256"]:
            raise RuntimeError(
                f"libghostty-vt archive digest mismatch: expected {entry['sha256']}, got {actual}. "
                "The tag archive is served from a cache that GitHub occasionally rebuilds; "
                "confirm the archive content before re-pinning."
            )
        partial.replace(archive)
    finally:
        partial.unlink(missing_ok=True)
    return archive


def extract(archive: Path, entry: dict) -> None:
    """Extract only the pinned paths so macOS assets and docs stay out of the tree."""
    staging = Path(tempfile.mkdtemp(dir=CACHE, prefix="libghostty-vt-"))
    try:
        with tarfile.open(archive) as tar:
            members = []
            for member in tar.getmembers():
                parts = member.name.split("/", 1)
                if len(parts) != 2:
                    continue
                relative = parts[1]
                if any(
                    relative == wanted or relative.startswith(wanted + "/")
                    for wanted in entry["paths"]
                ):
                    members.append(member)
            tar.extractall(staging, members=members, filter="data")
        extracted = next(iter(staging.iterdir()))
        if TARGET.exists():
            shutil.rmtree(TARGET)
        extracted.rename(TARGET)
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def stamp(entry: dict) -> None:
    (TARGET / ".libghostty-vt-source.json").write_text(
        json.dumps(
            {
                "tag": entry["tag"],
                "sha256": entry["sha256"],
                "paths": entry["paths"],
            },
            indent=2,
        )
        + "\n"
    )


def is_current(entry: dict) -> bool:
    record = TARGET / ".libghostty-vt-source.json"
    if not record.is_file() or not TARGET.is_dir():
        return False
    try:
        saved = json.loads(record.read_text())
    except json.JSONDecodeError:
        return False
    return saved.get("sha256") == entry["sha256"] and saved.get("paths") == entry["paths"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--force", action="store_true", help="re-extract an existing tree")
    parser.add_argument("--check", action="store_true", help="verify without extracting")
    args = parser.parse_args()

    entry = manifest()["libghostty_vt"]
    if args.check:
        archive = DOWNLOADS / f"libghostty-vt-{entry['tag']}.tar.gz"
        if not archive.is_file():
            print(f"missing cached archive {archive}", file=sys.stderr)
            return 1
        if digest(archive) != entry["sha256"]:
            print(f"cached archive does not match the pinned digest", file=sys.stderr)
            return 1
        if not is_current(entry):
            print(f"prepared tree is missing or stale: {TARGET}", file=sys.stderr)
            return 1
        print(TARGET)
        return 0

    if is_current(entry) and not args.force:
        print(TARGET)
        return 0
    CACHE.mkdir(parents=True, exist_ok=True)
    extract(download(entry), entry)
    stamp(entry)
    print(TARGET)
    return 0


if __name__ == "__main__":
    sys.exit(main())
