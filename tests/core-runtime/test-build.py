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


if __name__ == "__main__":
    unittest.main()
