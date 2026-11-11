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
        env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("HC_", "HERDR_"))
            and key not in {
                "DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR",
                "TMUX", "STY", "SSH_CONNECTION", "SSH_TTY", "VSCODE_IPC_HOOK_CLI",
            }
        }
        env.update(
            HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color",
            HISTFILE=str(home / "history"), PS1="copy$ ",
            HERDR_CORE_HOME=str(core), HERDR_CORE_BIN=str(UI.HC), HERDR_CORE_CLIENT_ID="copy-smoke",
            HC_SESSION="herdr-tui", HC_SOCKET=str(core / "core.sock"),
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
            viewer.send(b"printf '\\033[2J\\033[3J\\033[Halpha needle one\\nbeta needle two\\n\\nend marker\\nCOPY_READY\\n'\r")
            UI.until(lambda: "COPY_READY" in screen(), "known retained terminal text")
            viewer.send(b"\x1b[18~")  # F7: existing client copy-mode.
            UI.until(lambda: " COPY " in screen(), "copy-mode entry")
            viewer.send(b"/needle\r")
            UI.until(lambda: "1/2" in screen(), "endpoint-backed search result")
            viewer.send(b"y")
            UI.until(lambda: b"needle" in copied(), "search-match copy over captured OSC 52")
            viewer.send(b"\x1b[18~")
            UI.until(lambda: " COPY " in screen(), "copy-mode reentry")
            # The existing shell queues async word motions before the copy.
            viewer.send(b"g0vwey")
            UI.until(lambda: b"alpha needle" in copied(), "word motion + selection + copy pipeline")
            viewer.send(b"printf 'COPY_EXIT_%s\\n' OK\r")
            UI.until(lambda: "COPY_EXIT_OK" in screen(), "copy-mode exits to terminal input")
            assert hc("get", address)["pid"] == pid
            assert not (home / "forbidden-old.sock").exists()
            assert not (home / "forbidden-client.sock").exists()
            viewer.detach()
            print(json.dumps({
                "default_entry": True, "real_tui_copy_mode": True,
                "search_match_copy": True, "word_selection_pipeline": True,
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
            if (core / "core.sock").exists():
                subprocess.run(
                    [str(UI.HC), "server", "stop"], env=env,
                    capture_output=True, timeout=15, check=True,
                )


if __name__ == "__main__":
    main()
