#!/usr/bin/env python3
"""Exercise default-entry Core copy-mode through the real TUI in a private PTY.

This is a focused gate, NOT the whole Core-only migration acceptance suite.
Native desktop clipboard access is disabled; OSC 52 is captured, not executed.
No default Core/Herdr session, home, config, systemd unit or user clipboard is used.
"""
import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

SPEC = importlib.util.spec_from_file_location(
    "existing_ui_smoke", Path(__file__).with_name("existing-ui-smoke.py")
)
UI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(UI)
ORACLE = Path(os.environ.get(
    "CORE_UI_ORACLE", Path(__file__).parent / "target/debug/screen-oracle"
)).resolve()


def main():
    with tempfile.TemporaryDirectory(prefix="hc-copy-ui-", dir="/var/tmp") as scratch:
        project = Path(scratch) / "project"
        project.mkdir()
        os.chdir(project)
        home = Path(scratch)
        core = home / "core"
        env = UI.isolated_env(home, core)
        env.update(
            HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color",
            HISTFILE=str(home / "history"), PS1="copy$ ",
            CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(UI.HC), HERDR_CORE_CLIENT_ID="copy-smoke",
            HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12],
            XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(home / "state"),
            XDG_CONFIG_HOME=str(home / "config"),
            HERDR_CONFIG_PATH=str(home / "config.toml"),
            HERDR_SOCKET_PATH=str(home / "forbidden-old.sock"),
            HERDR_CLIENT_SOCKET_PATH=str(home / "forbidden-client.sock"),
        )
        (home / "config.toml").write_text('''onboarding = false
[experimental]
allow_nested = true
[keys]
copy_mode = "f7"
detach = "f10"
''')
        core_config = core / "config/herdr-core"
        core_config.mkdir(parents=True, mode=0o700)
        (core_config / "config.toml").write_text(
            'shell = ["/bin/bash", "--noprofile", "--norc"]\nnotes = false\n'
        )
        if not ORACLE.is_file():
            raise RuntimeError("screen-oracle missing; run just core-copy-smoke")
        viewer = None

        def hc(*args):
            result = subprocess.run(
                [str(UI.HC), "--json", *args], env=env, capture_output=True,
                text=True, timeout=15, check=True,
            )
            return json.loads(result.stdout)

        def screen():
            return subprocess.run(
                [str(ORACLE), "100", "28"], input=viewer.read(), capture_output=True,
                timeout=5, check=True,
            ).stdout.decode(errors="replace")

        def copied():
            return [
                base64.b64decode(match)
                for match in re.findall(rb"\x1b\]52;c;([^\x07\x1b]*)(?:\x07|\x1b\\)", viewer.read())
            ]

        try:
            viewer = UI.Viewer(env)
            UI.until(lambda: b"project" in viewer.read(), "default Core TUI")
            address = hc("ls")["terminals"][0]["address"]
            pid = hc("get", address)["pid"]
            viewer.send(b"stty -echo; printf 'ECHO_DISABLED\\n'\r")
            UI.until(lambda: "ECHO_DISABLED" in screen(), "quiet test shell")
            payload = (
                "\\033[2J\\033[3J\\033[Halpha needle one\\nbeta needle two\\n\\nend marker\\n"
                + "history filler\\n" * 80 + "COPY_READY\\n"
            )
            viewer.send(f"printf '{payload}'\r".encode())
            UI.until(lambda: "COPY_READY" in screen(), "known retained terminal text")
            viewer.send(b"\x1b[18~")  # F7: existing client copy-mode.
            UI.until(lambda: " COPY " in screen(), "copy-mode entry")
            viewer.send(b"/needle\r")
            UI.until(lambda: "1/2" in screen(), "endpoint-backed search result")
            UI.until(lambda: "alpha needle one" in screen(), "search scrolls into retained history")
            viewer.send(b"y")
            UI.until(lambda: b"needle" in copied(), "search-match copy over captured OSC 52")
            assert "unexpected" not in screen(), "pane.scroll must return pane info, not a generic acknowledgement"
            viewer.send(b"\x1b[18~")
            UI.until(lambda: " COPY " in screen(), "copy-mode reentry")
            # The existing shell queues async word motions before the copy.
            viewer.send(b"g0vwey")
            UI.until(lambda: b"alpha needle" in copied(), "word motion + selection + copy pipeline")
            assert "unexpected" not in screen(), "scrollback/copy must not raise an endpoint result error"
            viewer.send(b"printf 'COPY_EXIT_%s\\n' OK\r")
            UI.until(lambda: "COPY_EXIT_OK" in screen(), "copy-mode exits to terminal input")
            assert hc("get", address)["pid"] == pid
            assert not (home / "forbidden-old.sock").exists()
            assert not (home / "forbidden-client.sock").exists()
            viewer.detach()
            print(json.dumps({
                "default_entry": True, "real_tui_copy_mode": True,
                "search_match_copy": True, "word_selection_pipeline": True,
                "scrollback_endpoint_result": True,
                "exit_restores_terminal_input": True, "same_program_pid": True,
                "clipboard": "captured OSC52 only", "no_old_sockets": True,
                "full_migration_acceptance": False,
            }))
        except Exception:
            if viewer is not None:
                print("TUI screen:", screen())
                print("Copied payloads:", copied())
            raise
        finally:
            if viewer is not None:
                viewer.cleanup()
            socket = Path(env["XDG_RUNTIME_DIR"]) / "herdr-core" / f"{env['HC_SESSION']}.sock"
            if socket.exists():
                UI.stop_private_core(env)


if __name__ == "__main__":
    main()
