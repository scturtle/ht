#!/usr/bin/env python3
"""Pinned-source preparation contracts, using tiny local archives and no runtime."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

BUILDER = Path(__file__).resolve().parents[2] / "scripts/core-runtime/build.py"


class SourcePreparationTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="hc-build-contract-", dir="/var/tmp")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.here = self.root / "scripts/core-runtime"
        self.here.mkdir(parents=True)
        self.builder = self.here / "build.py"
        shutil.copy2(BUILDER, self.builder)
        inputs = self.root / "inputs"
        (inputs / "core/vendor/ghostty").mkdir(parents=True)
        (inputs / "core/README").write_text("old\n")
        (inputs / "ghostty").mkdir()
        (inputs / "ghostty/README").write_text("pinned Ghostty fixture\n")
        manifest = {"patches": ["fixture.patch"]}
        for name in ("core", "ghostty"):
            archive = self.root / f"{name}.tar.gz"
            with tarfile.open(archive, "w:gz") as output:
                output.add(inputs / name, arcname=name)
            manifest[name] = {"commit": "fixture", "url": archive.as_uri(),
                              "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}
        (self.here / "source.json").write_text(json.dumps(manifest))
        patches = self.here / "patches"
        patches.mkdir()
        (patches / "fixture.patch").write_text(
            "--- a/README\n+++ b/README\n@@ -1 +1 @@\n-old\n+new\n"
        )

    def prepare(self):
        return subprocess.run([sys.executable, str(self.builder), "prepare"],
                              capture_output=True, text=True, timeout=15)

    def test_fresh_prepare_stdout_is_only_the_verified_source_path(self):
        prepared = self.prepare()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertEqual(len(prepared.stdout.splitlines()), 1, prepared.stdout)
        source = Path(prepared.stdout.strip())
        self.assertEqual((source / "README").read_text(), "new\n")
        self.assertIn("patching file", prepared.stderr)

    def test_cached_prepare_is_stable_and_never_discards_edits(self):
        prepared = self.prepare()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        source = Path(prepared.stdout.splitlines()[-1])
        again = self.prepare()
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(again.stdout.strip(), str(source))
        (source / "README").write_text("local edits\n")
        refused = self.prepare()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("refusing to discard edits", refused.stderr)
        self.assertEqual((source / "README").read_text(), "local edits\n")

    def test_upgrading_a_pin_keeps_previous_sources_and_archives(self):
        first = self.prepare()
        self.assertEqual(first.returncode, 0, first.stderr)
        old_source = Path(first.stdout.strip())
        manifest_path = self.here / "source.json"
        manifest = json.loads(manifest_path.read_text())
        old_sha = manifest["core"]["sha256"]
        downloads = self.root / ".local/core-runtime/downloads"
        old_archive = downloads / f"core-{old_sha}.tar.gz"
        old_bytes = old_archive.read_bytes()
        (self.root / "inputs/core/new-file").write_text("new pin\n")
        upgraded = self.root / "core-v2.tar.gz"
        with tarfile.open(upgraded, "w:gz") as output:
            output.add(self.root / "inputs/core", arcname="core")
        new_sha = hashlib.sha256(upgraded.read_bytes()).hexdigest()
        manifest["core"] = {"commit": "fixture-v2", "url": upgraded.as_uri(), "sha256": new_sha}
        manifest_path.write_text(json.dumps(manifest))
        second = self.prepare()
        self.assertEqual(second.returncode, 0, second.stderr)
        new_source = Path(second.stdout.strip())
        self.assertNotEqual(old_source, new_source)
        self.assertEqual((old_source / "README").read_text(), "new\n")
        self.assertEqual((new_source / "new-file").read_text(), "new pin\n")
        self.assertEqual(old_archive.read_bytes(), old_bytes)
        self.assertEqual((downloads / f"core-{new_sha}.tar.gz").read_bytes(), upgraded.read_bytes())

    def test_matching_legacy_archive_is_reused_without_overwriting_it(self):
        downloads = self.root / ".local/core-runtime/downloads"
        downloads.mkdir(parents=True)
        legacy = downloads / "core.tar.gz"
        original = self.root / "core.tar.gz"
        original_bytes = original.read_bytes()
        legacy.write_bytes(original_bytes)
        original.unlink()  # Download would fail; the verified legacy cache must suffice.
        prepared = self.prepare()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertEqual(legacy.read_bytes(), original_bytes)
        sha = hashlib.sha256(original_bytes).hexdigest()
        self.assertEqual((downloads / f"core-{sha}.tar.gz").read_bytes(), original_bytes)

    def test_corrupt_pinned_archive_is_refused_not_overwritten(self):
        prepared = self.prepare()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        manifest_path = self.here / "source.json"
        manifest = json.loads(manifest_path.read_text())
        archive = self.root / ".local/core-runtime/downloads" / f"core-{manifest['core']['sha256']}.tar.gz"
        archive.write_bytes(b"corrupt cache")
        # Force a fresh bundle so preparation must read its archives again.
        manifest_path.write_text(manifest_path.read_text() + "\n")
        refused = self.prepare()
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("cached archive changed", refused.stderr)
        self.assertEqual(archive.read_bytes(), b"corrupt cache")


if __name__ == "__main__":
    unittest.main()
