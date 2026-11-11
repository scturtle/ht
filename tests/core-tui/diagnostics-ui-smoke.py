#!/usr/bin/env python3
"""Real menu capture, including a paused OWNED Core. Never uses the default session."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import tempfile
import time

SPEC = importlib.util.spec_from_file_location("ui", Path(__file__).with_name("existing-ui-smoke.py"))
UI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(UI)
ORACLE = (Path(__file__).parent / "target/debug/screen-oracle").resolve()


def main():
    with tempfile.TemporaryDirectory(prefix="hc-diagnostics-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        project = home / "project"
        project.mkdir()
        os.chdir(project)
        core = home / "core"
        session = UI.core_session(str(core))
        socket = core / "run/herdr-core" / f"{session}.sock"
        env = UI.isolated_env(home, core)
        env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color",
                   HISTFILE=str(home / "history"), CORE_TUI_HOME=str(core),
                   HERDR_CORE_BIN=str(UI.HC), HC_SESSION=session,
                   XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(core / "state"),
                   XDG_CONFIG_HOME=str(core / "config"), HERDR_CONFIG_PATH=str(home / "config.toml"))
        if "HERDR_ENV" in os.environ:
            env["HERDR_ENV"] = os.environ["HERDR_ENV"]
        (home / "config.toml").write_text('onboarding=false\n[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n')
        (core / "config/herdr-core").mkdir(parents=True)
        (core / "config/herdr-core/config.toml").write_text('shell=["/bin/bash","--noprofile","--norc"]\nnotes=false\n')
        viewer = None
        dumps = []
        stopped_pid = None
        moved_socket = None

        def hc(*args):
            result = subprocess.run([str(UI.HC), "--json", *args], env=env,
                                    capture_output=True, text=True, check=True, timeout=15)
            return json.loads(result.stdout)

        def screen():
            return subprocess.run([str(ORACLE.resolve()), "100", "28"], input=viewer.read(),
                                  capture_output=True, check=True, timeout=5).stdout.decode()

        def locate(label):
            for y, line in enumerate(screen().splitlines()):
                if label in line:
                    return line.index(label), y
            return None

        def click(point, button=0):
            x, y = point
            viewer.send(f"\x1b[<{button};{x+1};{y+1}M\x1b[<{button};{x+1};{y+1}m".encode())

        def capture(global_menu):
            old = set(Path("/var/tmp").glob(f"ht-pane-dump-{viewer.pid}-*"))
            if global_menu:
                click(UI.until(lambda: locate("menu"), "sidebar menu launcher"))
                label = "dump pane diagnostics"
            else:
                # Right click in the original pane, not the sidebar.
                click((70, 20), 2)
                label = "Dump pane diagnostics"
            click(UI.until(lambda: locate(label), "diagnostic menu entry"))

            def complete():
                viewer.read()
                for path in set(Path("/var/tmp").glob(f"ht-pane-dump-{viewer.pid}-*")) - old:
                    try:
                        report = json.loads((path / "report.json").read_text())
                    except (OSError, ValueError):
                        continue
                    return path, report
                return None
            path, report = UI.until(complete, "bounded diagnostic completion")
            dumps.append(path)
            assert stat.S_IMODE(path.stat().st_mode) == 0o700
            assert report["pane"] == address
            client = json.loads((path / "client.json").read_text())
            adapter = json.loads((path / "adapter.json").read_text())
            assert client["pane_id"] == address
            assert "DUMP_READY" in adapter["view"]["terminal"]["viewport"]["text"]
            UI.until(lambda: "Pane diagnostics" in screen(), "diagnostic save notice")
            return path, report, adapter

        try:
            viewer = UI.Viewer(env)
            UI.until(lambda: "project" in screen(), "real TUI first paint")
            address = hc("ls")["terminals"][0]["address"]
            viewer.send(b"stty -echo; printf 'DUMP_READY\\n'\r")
            UI.until(lambda: "DUMP_READY" in screen(), "stable pane text")
            before = hc("get", address)
            original_text = hc("read", address, "--lines", "0")["text"]
            doc = next((core / "state").rglob("docs.json"))
            original_doc = doc.read_bytes()
            path, report, adapter = capture(True)
            assert report["errors"] == [], report
            assert (path / "core-screen.txt").read_text() == original_text
            first_channel = adapter["view"]["channel"]
            core_state = json.loads((path / "core.json").read_text())
            core_pid = core_state["server.status"]["pid"]
            path, report, adapter = capture(False)
            assert report["errors"] == [], report
            assert adapter["view"]["channel"] == first_channel
            assert (path / "core-screen.txt").read_text() == original_text
            after = hc("get", address)
            assert all(before[k] == after[k] for k in ("pid", "cols", "rows"))
            assert doc.read_bytes() == original_doc

            # An absent socket must not start a replacement Core or leak relay
            # stderr JSON into the real TUI's terminal stream.
            moved_socket = socket.with_suffix(".capture-test")
            socket.rename(moved_socket)
            output_start = len(viewer.output)
            path, report, adapter = capture(True)
            assert report["errors"]
            assert not socket.exists(), "diagnostic probe auto-started Core"
            assert b'"kind":"unavailable"' not in viewer.output[output_start:]
            moved_socket.rename(socket)
            moved_socket = None
            assert hc("get", address)["pid"] == before["pid"]

            # Pause only the Core whose pid was read from OUR private socket.
            assert str(socket).startswith(str(home) + "/")
            own_env = Path(f"/proc/{core_pid}/environ").read_bytes()
            assert f"XDG_RUNTIME_DIR={core / 'run'}".encode() in own_env
            os.kill(core_pid, signal.SIGSTOP)
            stopped_pid = core_pid
            path, report, adapter = capture(True)
            assert report["errors"], "paused Core must produce an explicit partial capture"
            assert adapter["view"]["channel"] == first_channel
            captured_client=json.loads((path / "client.json").read_text())
            assert "pending_activation" not in captured_client["client_loop"]
            assert "queued" in captured_client["request_lane"]
            assert "\nState:\tT" in Path(f"/proc/{core_pid}/status").read_text(), "dump must not resume Core"
            os.kill(core_pid, signal.SIGCONT)
            stopped_pid = None
            assert hc("get", address)["pid"] == before["pid"]
            viewer.send(b"printf 'AFTER_DUMP_%s\\n' OK\r")
            UI.until(lambda: "AFTER_DUMP_OK" in screen(), "input survives diagnostic capture")
            viewer.detach()
            print(json.dumps({"global_menu_dump": True, "pane_context_dump": True,
                              "private_permissions": True, "same_program_pid_and_grid": True,
                              "same_attach_channel": True, "no_document_write": True,
                              "absent_socket_not_restarted": True, "relay_stderr_does_not_corrupt_tui": True,
                              "paused_core_partial_dump": True, "dump_does_not_resume_core": True,
                              "input_after_dump": True}))
        finally:
            if moved_socket is not None:
                if socket.exists():
                    UI.stop_private_core(env)
                moved_socket.rename(socket)
            if stopped_pid is not None:
                os.kill(stopped_pid, signal.SIGCONT)
            if viewer is not None:
                viewer.cleanup()
            if socket.exists():
                UI.stop_private_core(env)
            for path in dumps:
                shutil.rmtree(path)


if __name__ == "__main__":
    main()
