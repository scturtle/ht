#!/usr/bin/env python3
"""Real kitten icat must get PTY pixels and emit an image through ht/Core.

Unlike the raw-APC graphics gate, this exercises icat's TIOCGWINSZ prerequisite.
Uses only a private Core/holder; never stops or restarts the user's session.
Requires kitten (override its path with CORE_TUI_KITTEN).
"""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import tempfile

from harness import isolated_env, stop_private_core, until

SPEC = importlib.util.spec_from_file_location(
    "graphics_gate", Path(__file__).with_name("graphics-ui-smoke.py")
)
GRAPHICS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GRAPHICS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, help="use this image instead of a 1x1 PNG")
    args = parser.parse_args()
    kitten = os.environ.get("CORE_TUI_KITTEN") or shutil.which("kitten")
    if not kitten:
        raise SystemExit("kitten is required; set CORE_TUI_KITTEN to its absolute path")
    kitten = str(Path(kitten).resolve())
    with tempfile.TemporaryDirectory(prefix="hc-icat-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        env = isolated_env(home, home / "core")
        (home / "config.toml").write_text('[keys]\ndetach="f10"\n')
        for profile in (".bashrc", ".bash_profile"):
            (home / profile).write_text("PS1='ICAT_GATE_READY> '\n")
        image = args.image.resolve() if args.image else home / "pixel.png"
        if not args.image:
            image.write_bytes(base64.b64decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk"
                "+A8AAQUBAScY42YAAAAASUVORK5CYII="
            ))
        report, done, error = (home / name for name in ("winsize.json", "exit", "stderr"))
        probe = home / "winsize.py"
        probe.write_text(
            "import fcntl,json,struct,termios\n"
            "print(json.dumps(struct.unpack('HHHH',fcntl.ioctl(0,termios.TIOCGWINSZ,b'\\0'*8))))\n"
        )
        view = None
        try:
            view = GRAPHICS.Viewer(env)
            until(lambda: b"ICAT_GATE_READY>" in view.read(), "private prompt painted")
            q = lambda path: shlex.quote(str(path))
            view.send((
                f"python3 {q(probe)} >{q(report)}; "
                f"{q(kitten)} icat {q(image)} 2>{q(error)}; echo $? >{q(done)}\r"
            ).encode())
            until(lambda: (view.read(), done.exists())[1], "icat exited")
            geometry = json.loads(report.read_text())
            rows, cols, width, height = geometry
            assert width == cols * view.CELL_W and height == rows * view.CELL_H, geometry
            assert int(done.read_text()) == 0, error.read_text()

            def placed():
                frames = GRAPHICS.graphics_frames(view.read())
                return (any(c.startswith(("a=t,", "a=T,")) and p for c, p in frames)
                        and any("a=p," in c for c, _ in frames))

            until(placed, "icat image upload and placement reached host")
            view.send(b"\x1b[21~")
            view.wait_exit()
            print(json.dumps({"icat_exit": 0, "pty_geometry": geometry,
                              "image_reaches_host": True, "kitten": kitten}))
        finally:
            if view:
                view.cleanup()
            stop_private_core(env)


if __name__ == "__main__":
    main()
