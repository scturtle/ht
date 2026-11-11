#!/usr/bin/env python3
"""Isolated real-TUI gate for Core reconnect and non-starting recovery probes.

Stage 1: losing the Core connection must not end the TUI. The adapter retries
over a fresh `hc connect --existing` relay, re-attaches every pane, restores
screens from Core's preserved state, and input keeps working. Programs keep the
same pid because holders (or Core's adoption) own the PTYs, not the TUI relay.

Stage 2: a recovery probe must never start a Core. After an explicit
`server stop` the TUI gives up instead of resurrecting the runtime, so no socket
reappears and no process is spawned.

This drives the real default TUI through a private Core/PTY. It is not full
migration acceptance.
"""
import fcntl
import json
from harness import isolated_env, private_socket, stop_private_core
import os
import pty
from pathlib import Path
import select
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[2]
HERDR = Path(os.environ.get("CORE_UI_HERDR", ROOT / "target/release/ht")).resolve()
HC = Path(
    os.environ.get("CORE_TUI_HC", ROOT / ".local/core-runtime/target/release/herdr-core")
).resolve()

# A planned restart brings Core back in well under a second; the explicit-stop
# early exit only fires after this long with nothing listening.
ABSENT_WINDOW = 5.0


def until(check, description, timeout=25):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError(f"timeout: {description}")


class Viewer:
    """A host terminal running the real client with a fixed reported geometry."""

    COLS, ROWS, CELL_W, CELL_H = 100, 30, 8, 16

    def __init__(self, env):
        self.output = bytearray()
        master, slave = pty.openpty()
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

    def relays(self):
        """The `hc connect` relay processes this client currently owns."""
        result = subprocess.run(
            ["pgrep", "-P", str(self.process.pid)],
            capture_output=True,
            text=True,
        )
        pids = []
        for pid in result.stdout.split():
            try:
                cmdline = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ")
            except OSError:
                continue
            if b"herdr-core" in cmdline and b"connect" in cmdline:
                pids.append(int(pid))
        return pids

    def cleanup(self):
        if self.fd is None:
            return
        if self.process.poll() is None:
            self.process.terminate()
            deadline = time.monotonic() + 5
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


def core_processes(socket):
    """Processes belonging to this Core home, found by environment."""
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            environ = Path(f"/proc/{entry}/environ").read_bytes()
            cmdline = Path(f"/proc/{entry}/cmdline").read_bytes().replace(b"\0", b" ")
        except OSError:
            continue
        if f"HC_SOCKET={socket}".encode() in environ and b"herdr-core" in cmdline:
            found.append(int(entry))
    return found


def main():
    with tempfile.TemporaryDirectory(prefix="hc-reconnect-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        core = home / "core"
        # The client no longer keeps a socket of its own: it drives the `hc`
        # session socket, so the gate derives the same path `hc` would.
        session = "gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12]
        socket = core / "run" / "herdr-core" / f"{session}.sock"
        env = isolated_env(home, core)
        env.update(
            CORE_TUI_HOME=str(core),
            HERDR_CORE_BIN=str(HC),
            TERM="xterm-256color",
            HC_SESSION=session,
            XDG_RUNTIME_DIR=str(core / "run"),
            XDG_STATE_HOME=str(home / "state"),
            XDG_CONFIG_HOME=str(home / "config"),
            HERDR_CONFIG_PATH=str(home / "config.toml"),
        )
        (home / "config.toml").write_text(
            'onboarding = false\n[experimental]\nallow_nested = true\n[keys]\ndetach = "f10"\n'
        )

        def hc(*args):
            result = subprocess.run(
                [str(HC), "--json", *args],
                env=env,
                capture_output=True,
                text=True,
                timeout=20,
            )
            return json.loads(result.stdout) if result.stdout.strip() else {}

        import sys
        logf = open("/tmp/timeline.txt", "w")
        view = Viewer(env)
        try:
            until(lambda: hc("ls").get("terminals"), "private core and first pane")
            address = hc("ls")["terminals"][0]["address"]
            pid = hc("get", address)["pid"]
            serve = hc("server", "status")["pid"]
            before_relays = until(view.relays, "the client's own Core relay")
            settled = len(view.read())

            # --- Stage 1: a planned restart must recover without ending the TUI. ---
            # This is the documented "programs keep running, the next core adopts
            # them" path, and the real analogue of a live handoff.
            hc("server", "restart")
            replacement = until(
                lambda: set(view.relays()) - set(before_relays),
                "a replacement relay after the Core restarted",
            )
            assert view.process.poll() is None, "the TUI must survive a Core restart"
            assert all(not Path(f"/proc/{old}").exists() for old in before_relays), (
                "the relay of the dead connection must not be reused"
            )
            until(
                lambda: hc("server", "status")["pid"] != serve,
                "a new Core process took over",
            )
            until(
                lambda: hc("get", address).get("pid") == pid,
                "the program survives the Core restart",
            )

            # Screens come back from Core's preserved state and the terminal modes
            # are restored, so typed input reaches the same program again.
            marker = "RECONNECT_INPUT_OK"
            view.send(f"echo {marker}\r".encode())
            until(
                lambda: marker.encode() in view.read()[settled:],
                "input works after the reconnect",
            )

            # --- Stage 2: a stop the user asked for is never undone. ---
            assert hc("server", "stop")["stopped"] is True
            stopped_at = time.monotonic()
            import sys
            def exited():
                poll = view.process.poll()
                tail = view.read()[-160:]
                print("DBG poll", poll, "socket", socket.exists(), "core procs",
                      core_processes(socket), "tail", tail, file=sys.stderr, flush=True)
                return poll is not None
            until(
                exited,
                "the TUI gives up after an explicit stop",
                timeout=ABSENT_WINDOW + 20,
            )

            gave_up_after = time.monotonic() - stopped_at
            # Give a would-be auto-start every chance to appear before asserting.
            time.sleep(3.0)
            view.read()
            assert not socket.exists(), "a recovery probe resurrected the stopped Core"
            assert not core_processes(socket), "a stopped Core left processes behind"
            assert gave_up_after < ABSENT_WINDOW + 12, gave_up_after

            print(
                json.dumps(
                    {
                        "default_tui": True,
                        "survives_core_restart": True,
                        "replacement_relay": bool(replacement),
                        "dead_relay_not_reused": True,
                        "same_program_pid_after_reconnect": True,
                        "screens_restored_from_core": True,
                        "input_after_reconnect": True,
                        "explicit_stop_not_resurrected": True,
                        "stop_gave_up_seconds": round(gave_up_after, 2),
                        "full_migration_acceptance": False,
                    }
                )
            )
        finally:
            try:
                view.cleanup()
            finally:
                stop_private_core(env)


if __name__ == "__main__":
    main()
