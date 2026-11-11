#!/usr/bin/env python3
"""Isolated real TUI smoke for the initial adapter milestone, not full migration acceptance."""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
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


def argv(env):
    """The client takes its Core home and session as arguments, never from the
    environment."""
    home = env.get("CORE_TUI_HOME")
    if not home:
        return [str(HERDR)]
    return [str(HERDR), "--core-home", home, "--core-session", core_session(home)]


class Viewer:
    def __init__(self, env, binary=None, cols=100, rows=28):
        self.output = bytearray()
        self.resizes = [(0, cols, rows)]
        self.pid, self.fd = pty.fork()
        if not self.pid:
            # Set geometry before exec: a parent-side ioctl races terminal setup.
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            args=argv(env)
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


def main():
    with tempfile.TemporaryDirectory(prefix="hc-existing-ui-", dir="/var/tmp") as scratch:
        # The workspace name is derived from the root pane cwd (original behavior),
        # so run inside a fixed-name directory to keep the screen deterministic.
        project = Path(scratch) / "project"
        project.mkdir()
        os.chdir(project)
        home = Path(scratch)
        core = home / "core"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_"))}
        env.update(CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(HC), TERM="xterm-256color",
                   HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12],
                   XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(home / "state"),
                   XDG_CONFIG_HOME=str(home / "config"), HERDR_CONFIG_PATH=str(home / "config.toml"),
                   HERDR_SOCKET_PATH=str(home / "forbidden-old.sock"),
                   HERDR_CLIENT_SOCKET_PATH=str(home / "forbidden-client.sock"))
        (home / "config.toml").write_text('''onboarding = false
[experimental]
allow_nested = true
[keys]
split_vertical = "f2"
new_tab = "f3"
close_pane = "f4"
cycle_pane_next = "f6"
detach = "f10"
''')
        viewers = []

        def hc(*args):
            result = subprocess.run([str(HC), "--json", *args], env=env, capture_output=True, text=True, timeout=15, check=True)
            return json.loads(result.stdout)

        def terminals():
            return [hc("get", row["address"]) for row in hc("ls")["terminals"]]

        def pump(view, check):
            view.read()
            return check()

        try:
            viewer = Viewer(env)
            viewers.append(viewer)
            until(lambda: b"project" in viewer.read(), "real sidebar/tab first paint")
            first = until(lambda: terminals(), "first Core shell")[0]
            viewer.send(b"printf 'INTEGRATED_%s\\n' INPUT_OK\r")
            until(lambda: pump(viewer, lambda: "INTEGRATED_INPUT_OK" in hc("read", first["address"])["text"]), "keyboard through actual UI")
            until(lambda: b"INTEGRATED_INPUT_OK" in viewer.read(), "Ghostty cells drawn in real UI")
            viewer.send(b"\x1bOQ")  # F2: normal existing split action
            until(lambda: pump(viewer, lambda: len(terminals()) == 2), "split action creates second Core terminal")
            second = next(t for t in terminals() if t["address"] != first["address"])
            viewer.send(b"printf 'SECOND_%s\\n' FOCUS_OK; printf '\\033]7501;state=blocked:kind=question:app=core-smoke\\033\\\\'\r")
            until(lambda: pump(viewer, lambda: (hc("get", second["address"]).get("status") or {}).get("state") == "blocked"), "OSC 7501")
            until(lambda: b"SECOND_FOCUS_OK" in viewer.read(), "focused split renders")
            until(lambda: b"core-smoke" in viewer.read(), "OSC status rendered in real sidebar")
            viewer.send(b"\x1b[17~")  # F6: focus is an asynchronous existing endpoint action
            def focused_left():
                cursors = re.findall(rb"\x1b\[\d+;(\d+)H\x1b\[\?25h", viewer.read())
                return json.loads((core / "layout.json").read_text())["focused_pane_id"] == first["address"] and cursors and int(cursors[-1]) < 64
            until(focused_left, "focus acknowledged and painted")
            viewer.send(b"printf 'FIRST_%s\\n' REFOCUS_OK\r")
            until(lambda: pump(viewer, lambda: "FIRST_REFOCUS_OK" in hc("read", first["address"])["text"]), "stable focus/input target")
            before_size = {t["address"]: (t["cols"], t["rows"]) for t in terminals()}
            viewer.resize(120, 34)
            until(lambda: pump(viewer, lambda: all(t["cols"] > before_size[t["address"]][0] and t["rows"] > before_size[t["address"]][1] for t in terminals())), "both panes resized")
            viewer.send(b"\x1bOR")  # F3: existing new-tab name modal, then confirm.
            until(lambda: b"new tab" in viewer.read(), "existing create-tab modal")
            viewer.send(b"\r")
            until(lambda: b"tab.create" in re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", viewer.read()), "unsupported action notice")
            assert len(terminals()) == 2
            duplicate = Viewer(env)
            viewers.append(duplicate)
            duplicate.wait_exit(success=False)
            assert b"already open" in duplicate.output
            before = {t["address"]: t["pid"] for t in terminals()}
            # Core is the snapshot authority; Bash can redraw its recent prompt
            # lines on SIGWINCH. Verify replay against content still in Core.
            assert "INTEGRATED_INPUT_OK" in hc("read", first["address"])["text"]
            saved = json.loads((core / "layout.json").read_text())
            viewer.detach()
            assert {t["address"]: t["pid"] for t in terminals()} == before
            resumed = Viewer(env)
            viewers.append(resumed)
            until(lambda: b"project" in resumed.read(), "relaunch into real TUI")
            until(lambda: b"INTEGRATED_INPUT_OK" in resumed.read(), "Core screen/history replay")
            assert {t["address"]: t["pid"] for t in terminals()} == before
            restored = json.loads((core / "layout.json").read_text())
            assert restored == saved, (saved, restored)
            resumed.send(b"\x1bOS")  # F4: existing close-pane action targets the restored focus.
            until(lambda: pump(resumed, lambda: len([t for t in terminals() if t["state"] == "live"]) == 1), "close only the targeted Core program")
            remaining = [t for t in terminals() if t["state"] == "live"]
            assert remaining[0]["address"] == second["address"] and remaining[0]["pid"] == before[second["address"]]
            resumed.detach()
            assert not (home / "forbidden-old.sock").exists()
            assert not (home / "forbidden-client.sock").exists()
            print(json.dumps({"existing_tui": True, "split": True, "focus_input": True,
                              "ghostty_render": True, "osc7501": True, "resize": True,
                              "unsupported_notice": True, "exclusive_client": True,
                              "restore_layout_and_pids": True, "close_target_only": True, "no_old_sockets": True}))
        except Exception:
            for viewer in viewers:
                if viewer.pid:
                    viewer.read()
                plain = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x1b]*(?:\x07|\x1b\\)", b"", bytes(viewer.output))
                print("TUI capture:", plain.decode(errors="replace")[-12000:])
            raise
        finally:
            for viewer in viewers:
                viewer.cleanup()
            subprocess.run([str(HC), "server", "stop"], env=env, capture_output=True, timeout=15)


if __name__ == "__main__":
    main()
