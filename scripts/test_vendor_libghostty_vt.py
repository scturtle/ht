from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


class VendorLibghosttyVtTests(unittest.TestCase):
    def test_vendored_tree_contains_required_upstream_files(self) -> None:
        root = Path(__file__).resolve().parent.parent / ".local" / "libghostty-vt"
        if not (root / "build.zig").is_file():
            self.skipTest("libghostty-vt source is not fetched; run scripts/fetch_libghostty_vt.py")
        # The prepared tree is the pristine upstream archive. VERSION and
        # LIB_VERSION are written into the staged copy by the build script.
        required = [
            root / "build.zig",
            root / "build.zig.zon",
            root / "include" / "ghostty" / "vt.h",
            root / "include" / "ghostty" / "vt" / "render.h",
            root / "src" / "lib_vt.zig",
            root / "dist" / "cmake" / "ghostty-vt-config.cmake.in",
        ]

        missing = [str(path.relative_to(root)) for path in required if not path.exists()]
        self.assertEqual(missing, [])

    def test_vendor_metadata_exists_and_points_at_vendored_tree(self) -> None:
        project_root = Path(__file__).resolve().parent.parent
        metadata = project_root / "vendor" / "libghostty-vt.vendor.json"
        self.assertTrue(metadata.exists())
        text = metadata.read_text()
        self.assertIn('"source_commit"', text)
        self.assertIn('"dist_archive"', text)
        self.assertIn('"extracted_dir"', text)

    def test_local_vendor_patches_are_listed_in_patch_index(self) -> None:
        project_root = Path(__file__).resolve().parent.parent
        index = project_root / "vendor" / "libghostty-vt.patches.md"
        patch_dir = project_root / "vendor" / "patches" / "libghostty-vt"
        patches = sorted(patch_dir.glob("*.patch"))

        if not patches:
            return

        self.assertTrue(index.exists())
        text = index.read_text()
        missing = [
            path.relative_to(project_root).as_posix()
            for path in patches
            if path.relative_to(project_root).as_posix() not in text
        ]
        self.assertEqual(missing, [])

    def test_local_vendor_patches_apply_to_the_pinned_archive(self) -> None:
        # The fetched tree stays pristine, so patches are applied to a staged copy
        # during the build. Verify each one still applies to a clean export of the
        # pinned archive instead of reverse-applying against a patched tree.
        project_root = Path(__file__).resolve().parent.parent
        patch_dir = project_root / "vendor" / "patches" / "libghostty-vt"
        source = project_root / ".local" / "libghostty-vt"

        patches = sorted(patch_dir.glob("*.patch"))
        if not patches:
            return
        if not (source / "build.zig").is_file():
            self.skipTest("libghostty-vt source is not fetched")

        with tempfile.TemporaryDirectory() as temp_dir:
            staged = Path(temp_dir) / "staged"
            staged.mkdir()
            for entry in ("build.zig", "build.zig.zon", "include", "src", "pkg"):
                origin = source / entry
                if origin.is_dir():
                    shutil.copytree(origin, staged / entry)
                elif origin.is_file():
                    shutil.copy2(origin, staged / entry)

            for patch in patches:
                with patch.open("rb") as handle:
                    result = subprocess.run(
                        ["patch", "-p1", "--forward", "--dry-run"],
                        cwd=staged,
                        stdin=handle,
                        text=False,
                        capture_output=True,
                    )
                self.assertEqual(
                    result.returncode,
                    0,
                    f"{patch.relative_to(project_root)} no longer applies to the pinned "
                    f"archive tree:\n{result.stdout.decode(errors='replace')}",
                )

    def test_embedded_libghostty_logging_is_silenced(self) -> None:
        root = Path(__file__).resolve().parent.parent / ".local" / "libghostty-vt"
        if not (root / "src" / "lib_vt.zig").is_file():
            self.skipTest("libghostty-vt source is not fetched")
        lib_vt = root / "src" / "lib_vt.zig"
        sys_zig = root / "src" / "terminal" / "c" / "sys.zig"
        lib_text = lib_vt.read_text()
        sys_text = sys_zig.read_text()
        self.assertIn('.logFn = @import("terminal/c/sys.zig").logFn', lib_text)
        self.assertIn("if (global.log == null) return;", sys_text)


if __name__ == "__main__":
    unittest.main()
