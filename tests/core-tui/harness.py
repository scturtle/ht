#!/usr/bin/env python3
"""Shared private Core/PTY selectors, geometry, and exact owned cleanup for TUI gates."""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import time

ROOT = Path(__file__).resolve().parents[2]
HERDR = Path(os.environ.get("CORE_UI_HERDR", ROOT / "target/release/ht")).resolve()
HC = Path(os.environ.get("CORE_TUI_HC", ROOT / ".local/core-runtime/target/release/herdr-core")).resolve()


def until(check, description):
    deadline = time.monotonic() + 18
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError(f"timeout: {description}")


def core_session(home):
    """A private session name per isolated home, so a gate never drives the
    session a person is working in."""
    import hashlib

    return "gate-" + hashlib.sha1(str(home).encode()).hexdigest()[:12]


def isolated_env(home, core):
    """One selector policy for all private gates; preserve only the nesting marker."""
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("HC_", "HERDR_", "PI_", "CORE_UI_", "CORE_TUI_"))
           and k not in {"DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR",
                        "TMUX", "TMUX_PANE", "STY", "SSH_CONNECTION", "SSH_TTY",
                        "VSCODE_IPC_HOOK_CLI"}}
    if "HERDR_ENV" in os.environ:
        env["HERDR_ENV"] = os.environ["HERDR_ENV"]
    home, core = Path(home), Path(core)
    env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color",
               CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(HC), HC_SESSION=core_session(core),
               XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(home / "state"),
               XDG_CONFIG_HOME=str(home / "config"), HERDR_CONFIG_PATH=str(home / "config.toml"))
    return env


def private_socket(env):
    core = Path(env["CORE_TUI_HOME"])
    assert env["HC_SESSION"] == core_session(core), "not a gate-owned session"
    assert Path(env["XDG_RUNTIME_DIR"]) == core / "run"
    socket = core / "run/herdr-core" / (core_session(core) + ".sock")
    assert not env.get("HC_SOCKET") or Path(env["HC_SOCKET"]) == socket
    return socket


def stop_private_core(env):
    socket = private_socket(env)
    if socket.exists():
        subprocess.run([str(HC), "server", "stop"], env=env, capture_output=True,
                       text=True, timeout=15, check=True)
        until(lambda: not socket.exists(), "owned Core socket removed")


def argv(env):
    private_socket(env)  # Never let a test fall back to the human's default.
    home=env["CORE_TUI_HOME"]
    return [str(HERDR), "--core-home", home, "--core-session", core_session(home)]


class Viewer:
    def __init__(self, env, binary=None, cols=100, rows=28):
        self.output = bytearray()
        self.resizes = [(0, cols, rows)]
        args=argv(env)
        self.pid, self.fd = pty.fork()
        if not self.pid:
            # Set geometry before exec: a parent-side ioctl races terminal setup.
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            args[0]=str(binary or HERDR)
            os.execve(args[0], args, env)

    def resize(self, cols, rows):
        while select.select([self.fd], [], [], 0)[0]:
            self.read()
        self.resizes.append((len(self.output), cols, rows))
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def read(self):
        if select.select([self.fd], [], [], 0.03)[0]:
            try:
                self.output.extend(os.read(self.fd, 65536))
            except OSError:
                pass
        return bytes(self.output)

    def send(self, data):
        os.write(self.fd, data)

    def wait_exit(self, success=True):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            self.read()
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.pid = None
                code = os.waitstatus_to_exitcode(status)
                assert (code == 0) == success, (code, bytes(self.output))
                os.close(self.fd)
                return
        raise AssertionError("viewer did not exit")

    def detach(self):
        self.send(b"\x1b[21~")
        self.wait_exit()

    def cleanup(self):
        if self.pid:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            # SIGTERM is handled by the TUI; drain its PTY so a blocked renderer
            # can finish. Bound cleanup even when the child cannot shut down.
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                self.read()
                pid, _ = os.waitpid(self.pid, os.WNOHANG)
                if pid:
                    break
            else:
                try:
                    os.kill(self.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                os.waitpid(self.pid, 0)
            self.pid = None
            os.close(self.fd)


