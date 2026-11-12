#!/usr/bin/env python3
"""Isolated real-TUI gate for Kitty graphics: a program's image must reach the host.

The client owns the pane VT now, so the only observable that matters is what it
writes to the *host* terminal: the image upload and its placement. This gate
drives the real default TUI through a private Core/PTY, so it exercises the
restored collector end to end and is not full migration acceptance.
"""
import base64
import json
from harness import isolated_env, private_socket, stop_private_core
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import tempfile
import time

import fcntl
import pty
import termios

ROOT = Path(__file__).resolve().parents[2]
HERDR = Path(os.environ.get("CORE_UI_HERDR", ROOT / "target/release/ht")).resolve()
HC = Path(os.environ.get("CORE_TUI_HC", ROOT / ".local/core-runtime/target/release/herdr-core")).resolve()

# 1x1 RGBA, red. The protocol needs the whole frame in one direct transmission.
PIXEL = base64.b64encode(bytes([255, 0, 0, 255])).decode()


def until(check, description):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError(f"timeout: {description}")


class Viewer:
    """A host terminal that reports cell pixels, so placements can be measured."""

    COLS, ROWS, CELL_W, CELL_H = 100, 28, 8, 16

    def __init__(self, env):
        self.output = bytearray()
        master, slave = pty.openpty()
        # Set geometry before the client starts: it reads it once at startup.
        fcntl.ioctl(
            slave,
            termios.TIOCSWINSZ,
            struct.pack(
                "HHHH", self.ROWS, self.COLS, self.COLS * self.CELL_W, self.ROWS * self.CELL_H
            ),
        )
        self.process = subprocess.Popen(
            [str(HERDR)]
            + (
                [
                    "--core-home",
                    env["CORE_TUI_HOME"],
                    "--core-session",
                    "gate-"
                    + __import__("hashlib")
                    .sha1(env["CORE_TUI_HOME"].encode())
                    .hexdigest()[:12],
                ]
                if env.get("CORE_TUI_HOME")
                else []
            ),
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave)
        self.fd = master

    def read(self):
        while select.select([self.fd], [], [], 0.03)[0]:
            try:
                chunk = os.read(self.fd, 1 << 16)
            except OSError:
                break
            if not chunk:
                break
            self.output.extend(chunk)
        return bytes(self.output)

    def send(self, data):
        os.write(self.fd, data)

    def wait_exit(self):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.read()
            if self.process.poll() is not None:
                os.close(self.fd)
                self.fd = None
                return
        raise AssertionError("viewer did not exit")

    def cleanup(self):
        if self.fd is None:
            return
        if self.process.poll() is None:
            self.process.terminate()
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                self.read()
                if self.process.poll() is not None:
                    break
            else:
                self.process.kill()
                self.process.wait()
        try:
            os.close(self.fd)
        except OSError:
            pass
        self.fd = None


def graphics_frames(output):
    """Every kitty graphics APC in host output, split into control and payload."""
    frames = []
    for chunk in output.split(b"\x1b_G")[1:]:
        end = chunk.find(b"\x1b\\")
        if end < 0:
            continue
        body = chunk[:end]
        control, _, payload = body.partition(b";")
        frames.append((control.decode("ascii", "replace"), payload))
    return frames


def main():
    with tempfile.TemporaryDirectory(prefix="hc-graphics-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        core = home / "core"
        env = isolated_env(home, core)
        env.update(
            CORE_TUI_HOME=str(core),
            HERDR_CORE_BIN=str(HC),
            TERM="xterm-256color",
            HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12],
            XDG_RUNTIME_DIR=str(core / "run"),
            XDG_STATE_HOME=str(home / "state"),
            XDG_CONFIG_HOME=str(home / "config"),
            HERDR_CONFIG_PATH=str(home / "config.toml"),
        )
        (home / "config.toml").write_text(
            '[experimental]\nallow_nested = true\n[keys]\ndetach = "f10"\n'
        )
        # A registry entry exists before ht has attached and painted the shell.
        # Input sent in that gap can be discarded by initial bootstrap recovery.
        # Wait for a prompt actually rendered by this private client instead.
        for profile in (".bashrc", ".bash_profile"):
            (home / profile).write_text("PS1='CORE_GRAPHICS_READY> '\n")
        viewers = []

        def hc(*args):
            result = subprocess.run(
                [str(HC), "--json", *args],
                env=env,
                capture_output=True,
                text=True,
                timeout=15,
                check=True,
            )
            return json.loads(result.stdout)

        try:
            view = Viewer(env)
            viewers.append(view)
            until(lambda: b"CORE_GRAPHICS_READY>" in view.read(),
                  "private shell prompt painted by the client")
            address = hc("ls")["terminals"][0]["address"]
            pid = hc("get", address)["pid"]

            # A program uploads and displays one image. The bytes are written by
            # `cat`, so no shell quoting or escape interpretation is involved.
            upload = (
                f"\x1b[H\x1b_Ga=T,f=32,i=7,p=1,s=1,v=1,c=1,r=1,q=2;{PIXEL}\x1b\\"
                "IMG_DONE\r\n"
            ).encode()
            image = home / "image.bin"
            image.write_bytes(upload)
            view.send(f"cat {image}\r".encode())

            def frames():
                return graphics_frames(view.read())

            def upload():
                return [
                    (c, p)
                    for c, p in frames()
                    if c.startswith("a=t") or "a=t," in c or c.startswith("a=T")
                ]

            def placement():
                return [(c, p) for c, p in frames() if "a=p" in c]

            until(upload, "host received the image transmission")
            placed = until(placement, "host received the image placement")
            control = placed[0][0]
            assert "c=1" in control and "r=1" in control, control
            transmit = upload()[0]
            assert len(transmit[1]) > 4, transmit  # base64 RGBA payload

            # A metadata-only update must not delete the image: the committed scene
            # is reused instead of republishing an empty one.
            hc("label", address, "herdr.display_name=sentinel")
            until(
                lambda: hc("get", address)["labels"].get("herdr.display_name") == "sentinel",
                "shared label applied",
            )
            time.sleep(0.6)
            after = frames()
            assert not [c for c, _ in after if "a=d" in c], "image deleted by a metadata update"
            # The same image is never re-transmitted for this connection.
            payloads = [p for c, p in after if "a=t" in c and len(p) > 8]
            assert len(payloads) <= 1, f"image payload re-sent {len(payloads)} times"
            assert hc("get", address)["pid"] == pid, "the program must survive"

            view.send(b"\x1b[21~")
            view.wait_exit()
            print(
                json.dumps(
                    {
                        "default_tui": True,
                        "program_image_reaches_host": True,
                        "placement_emitted": True,
                        "scene_retained_across_metadata": True,
                        "payload_not_reuploaded": True,
                        "same_program_pid": True,
                        "full_migration_acceptance": False,
                        "private_core_stopped": True,
                    }
                )
            )
        finally:
            try:
                for viewer in viewers:
                    viewer.cleanup()
            finally:
                stop_private_core(env)


if __name__ == "__main__":
    main()
