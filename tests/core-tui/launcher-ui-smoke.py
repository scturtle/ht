#!/usr/bin/env python3
"""Exercise the actual user launcher, keyboard -> shell -> visible TUI -> reattach."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
LAUNCHER = ROOT / "scripts/core-tui/run.sh"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


UI = load("launcher_existing_ui", HERE / "existing-ui-smoke.py")
ORG = load("launcher_organization_ui", HERE / "organization-ui-smoke.py")
# Unlike earlier smokes, exercise the exact shell/Python entry point given to users.
UI.HERDR = LAUNCHER


def main():
    private = Path(tempfile.mkdtemp(prefix="hc-launcher-ui-", dir="/var/tmp"))
    core = private / "core"
    config = core / "config/herdr-core"
    config.mkdir(parents=True, mode=0o700)
    (config / "config.toml").write_text('shell = ["/bin/bash", "--noprofile", "--norc"]\nnotes = false\n')
    trial_config = private / "ui.toml"
    shell = private / "configured-shell"
    shell.write_text('#!/bin/sh\nprintf "CONFIGURED_SHELL_OK\\n"\nexec /bin/bash --noprofile --norc\n')
    shell.chmod(0o700)
    trial_config.write_text((ROOT / "scripts/core-tui/try-config.toml").read_text()
                            + '\n[terminal]\ndefault_shell = ' + json.dumps(str(shell))
                            + '\n[ui]\nprompt_new_workspace_name = true\n')
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_", "CORE_UI_")) and k not in {
        "DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR", "TMUX", "STY", "SSH_CONNECTION", "SSH_TTY", "VSCODE_IPC_HOOK_CLI"}}
    # The launcher performs a cached build; reuse toolchains, not the user's HOME/config/runtime.
    env.setdefault("CARGO_HOME", str(Path.home() / ".cargo"))
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    env.update(HOME=str(private), SHELL="/bin/bash", TERM="xterm-256color", HISTFILE=str(private / "history"),
               CORE_TUI_HC=str(UI.HC), CORE_UI_HOME=str(core), CORE_UI_CONFIG=str(trial_config),
               HC_SESSION="herdr-tui", HC_SOCKET=str(core / "core.sock"),
               XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(core / "state"), XDG_CONFIG_HOME=str(core / "config"))
    viewers, rpc, result = [], None, None
    try:
        current = UI.Viewer(env)
        viewers.append(current)

        def screen():
            return subprocess.run([str(ORG.ORACLE), "100", "28", json.dumps(current.resizes)],
                                  input=current.read(), capture_output=True, timeout=5, check=True).stdout.decode(errors="replace")

        UI.until(lambda: "CONFIGURED_SHELL_OK" in screen(), "actual launcher first screen")
        rpc = ORG.Rpc(env)

        def terminals():
            current.read()
            reply = rpc.request("terminal.list", {})
            assert "error" not in reply, reply
            return reply["result"]["terminals"]

        def marker(name):
            # The completed marker cannot appear merely from the shell echoing this command.
            current.send(f"printf 'LAUNCH_%s\\n' '{name}'\r".encode())
            UI.until(lambda: f"LAUNCH_{name}" in screen(), f"actual launcher keyboard executes {name}")

        marker("FIRST_INPUT")
        assert "CONFIGURED_SHELL_OK" in screen(), "initial shell must honor terminal.default_shell"

        def assert_host_mouse_enabled():
            modes = {}
            for codes, enabled in re.findall(rb"\x1b\[\?([0-9;]+)([hl])", current.read()):
                for code in codes.split(b";"):
                    modes[int(code)] = enabled == b"h"
            assert any(modes.get(n, False) for n in (1000, 1002, 1003)), ("TUI mouse reporting was disabled by a plain shell", modes)
            assert modes.get(1006, False), ("SGR mouse encoding must stay enabled", modes)

        assert_host_mouse_enabled()
        initial = terminals()[0]
        clients = list((core / "clients").iterdir())
        assert len(clients) == 1 and (clients[0] / "client.lock").exists(), "stdin must retain its TTY identity"
        def preferences():
            return json.loads((clients[0] / "preferences.json").read_text())

        def active_tab():
            p = preferences()
            return p["active_tabs"][p["active_workspace"]]

        def focused():
            return preferences()["focus"][active_tab()]

        def mouse(code, x, y, down=True):
            # Do not bypass host-mode negotiation by injecting mouse while reporting is off.
            assert_host_mouse_enabled()
            current.send(f"\x1b[<{code};{x};{y}{'M' if down else 'm'}".encode())

        def click(x, y):
            mouse(0, x, y)
            mouse(0, x, y, False)

        current.send(b"\x02v")
        UI.until(lambda: len(terminals()) == 2, "actual launcher Ctrl+b v split")
        marker("SECOND_INPUT")
        first_tab = active_tab()
        second_pane = focused()
        current.send(b"\x02\t")
        UI.until(lambda: focused() == initial["address"], "Ctrl+b Tab focuses first pane")
        current.send(b"\x02\t")
        UI.until(lambda: focused() == second_pane, "Ctrl+b Tab returns to second pane")
        click(35, 8)
        UI.until(lambda: focused() == initial["address"], "mouse focuses left pane")
        marker("MOUSE_FOCUS")

        def first_root():
            return rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"]

        original_ratio = first_root()["ratio"]
        mouse(0, 64, 14)
        mouse(32, 74, 14)
        mouse(0, 74, 14, False)
        UI.until(lambda: first_root()["ratio"] != original_ratio, "mouse drags split boundary")
        current.send(b"for n in $(seq 1 60); do printf 'SCROLL_ROW_%03d\\n' $n; done\r")
        UI.until(lambda: "SCROLL_ROW_060" in screen(), "scroll history fixture")
        for _ in range(4):
            mouse(64, 35, 8)
        UI.until(lambda: "SCROLL_ROW_060" not in screen(), "mouse wheel scrolls retained history")
        for _ in range(4):
            mouse(65, 35, 8)
        UI.until(lambda: "SCROLL_ROW_060" in screen(), "mouse wheel returns to bottom")

        # Real tab-bar controls, not a keyboard substitute for the click.
        tab_plus = screen().splitlines()[0].index("+") + 1
        click(tab_plus, 1)
        UI.until(lambda: "new tab" in screen().lower(), "mouse opens new-tab dialog")
        current.send(b"MouseTab\r")
        UI.until(lambda: active_tab() != first_tab, "mouse-created tab becomes active")
        UI.until(lambda: "MouseTab" in screen(), "new tab displayed")
        current.send(b"\x1bOQ")
        UI.until(lambda: len(terminals()) == 4, "actual launcher F2 split in new tab")
        click(screen().splitlines()[0].index("1", 26) + 1, 1)
        UI.until(lambda: active_tab() == first_tab, "mouse switches back to original tab")
        marker("AFTER_TAB_CLICK")
        before = {t["address"]: t["pid"] for t in terminals()}
        assert before[initial["address"]] == initial["pid"]
        current.detach()
        current = UI.Viewer(env)
        viewers.append(current)
        UI.until(lambda: "LAUNCH_AFTER_TAB_CLICK" in screen(), "actual launcher reattach")
        marker("REATTACHED_INPUT")
        assert {t["address"]: t["pid"] for t in terminals()} == before

        def tabs():
            return [tab for workspace in rpc.doc()["value"]["workspaces"] for tab in workspace["tabs"]]

        def tab_label(tab_id):
            return next(tab["label"] for tab in tabs() if tab["id"] == tab_id)

        def new_tab(label):
            previous = active_tab()
            current.send(b"\x02c")
            UI.until(lambda: "new tab" in screen().lower(), "new-tab naming dialog")
            current.send((label or "").encode() + b"\r")
            UI.until(lambda: active_tab() != previous, "new named tab becomes active")
            UI.until(lambda: "new tab" not in screen().lower(), "creation response closes naming overlay")
            marker("NEW_TAB_READY")
            return active_tab()

        def exit_focused():
            address = focused()
            closing_marker = "X_" + address
            marker(closing_marker)
            current.send(b"exit\r")
            UI.until(lambda: address not in [a for tab in tabs() for a in ORG.leaves(tab["root"])], "exit retires only its pane")
            UI.until(lambda: not tabs() or focused() in [a for tab in tabs() for a in ORG.leaves(tab["root"])], "exit repairs local focus")
            screen()
            # The host can release Enter (or lose focus) after Core has already
            # removed its pane. These are cleanup, not a paste into a new pane.
            if tabs():
                # Core publication/preferences are not a client focus barrier.
                # Observe removal of the old target before any fresh input.
                UI.until(lambda: f"LAUNCH_{closing_marker}" not in screen() and "bash-5.2$" in screen(),
                         "exit surface removes the old input target")
                current.send(b"\x1b[13;1:3u\x1b[O\x1b[I")
                marker("AFTER_CLOSE_CLEANUP_" + address)
                assert "Paste rejected" not in screen(), "closing must not reject a stale key release as paste"
                assert "Core pane not found" not in screen(), "closed-pane cleanup must be idempotent"
            return address

        named = new_tab("研发-Tab")
        assert tab_label(named) == "研发-Tab", "typed Unicode name must be stored exactly"
        exit_focused()
        assert named not in [tab["id"] for tab in tabs()], "last pane exit closes tab"
        expected = str(len(tabs()) + 1)
        numeric = new_tab(None)
        assert tab_label(numeric) == expected, "default name uses position, not lifetime counter"
        assert not next(tab for tab in tabs() if tab["id"] == numeric)["custom_label"], "unchanged original suggestion stays automatic"
        current.send(b"\x02v")
        UI.until(lambda: len(ORG.leaves(next(tab for tab in tabs() if tab["id"] == numeric)["root"])) == 2, "exit split fixture")
        exit_focused()
        assert numeric in [tab["id"] for tab in tabs()], "one pane exit must retain its surviving split"
        exit_focused()
        assert numeric not in [tab["id"] for tab in tabs()]
        assert all(t["argv"][0] == str(shell) for t in terminals()), "new tabs/splits must honor configured shell too"
        # Original close actions, not only shell exit: the surviving pane remains usable.
        before_close = sum(len(ORG.leaves(tab["root"])) for tab in tabs())
        marker("CLOSEME")
        current.send(b"\x02x")
        UI.until(lambda: sum(len(ORG.leaves(tab["root"])) for tab in tabs()) == before_close - 1, "original close-pane action")
        UI.until(lambda: "LAUNCH_CLOSEME" not in screen() and "bash-5.2$" in screen(),
                 "close surface removes the old input target")
        current.send(b"\x1b[O\x1b[I")
        marker("AFTER_EXPLICIT_PANE_CLOSE")
        assert "Paste rejected" not in screen() and "Core pane not found" not in screen()
        while tabs():
            exit_focused()
        current.wait_exit()
        assert b"\x1b[?1049l" in current.output, "last natural exit restores outer terminal"
        assert not rpc.doc()["value"]["workspaces"], "Core stays available, with no phantom members"
        closed = set(rpc.doc()["value"]["retiring"])
        for shortcut, description in [(b"\x02X", "tab"), (b"\x02x", "pane")]:
            # Explicit user launch starts a fresh shell via the same create action,
            # not reconnect/reseed/adoption of the previously closed programs.
            current = UI.Viewer(env)
            viewers.append(current)
            UI.until(lambda: "CONFIGURED_SHELL_OK" in screen(), "user relaunch after final close")
            assert len(tabs()) == 1 and tab_label(tabs()[0]["id"]) == "1"
            assert set(ORG.leaves(tabs()[0]["root"])).isdisjoint(closed)
            marker("REOPEN_" + description.upper())
            current.send(shortcut)
            UI.until(lambda: "close" in screen().lower() and "cancel" in screen().lower(), "original last-" + description + " confirmation")
            current.send(b"\r")
            current.wait_exit()
            assert b"\x1b[?1049l" in current.output
            assert not rpc.doc()["value"]["workspaces"]
            closed = set(rpc.doc()["value"]["retiring"])
        result = {"actual_user_launcher": True, "keyboard_executes_shell_commands": True,
                  "visible_output": True, "tty_identity_preserved": True, "f2_split": True,
                  "ctrl_b_split_and_focus": True, "host_mouse_reporting": True,
                  "mouse_pane_focus": True, "mouse_split_drag": True, "mouse_wheel": True,
                  "mouse_tab_create_and_switch": True, "configured_shell": True,
                  "exact_typed_and_suggested_tab_names": True, "exit_closes_panes_and_empty_tabs": True,
                  "original_last_pane_exits_tui": True, "explicit_last_tab_and_pane_close_exit": True,
                  "close_cleanup_does_not_reject_or_retarget_input": True, "retirements_survive_relaunch": True,
                  "same_program_pids_after_detach": True, "input_after_reattach": True,
                  "full_migration_acceptance": False}
    except BaseException:
        for viewer in viewers:
            print(bytes(viewer.output)[-5000:], file=sys.stderr)
        print(f"Preserved launcher reproduction: {private}", file=sys.stderr)
        raise
    finally:
        try:
            for viewer in viewers:
                viewer.cleanup()
        finally:
            try:
                if rpc:
                    rpc.close()
            finally:
                # Stop only this test's explicitly selected private runtime, via the same launcher.
                subprocess.run([str(LAUNCHER), "--stop"], env=env,
                               capture_output=True, timeout=15, check=True)
                deadline = time.monotonic() + 5
                while (core / "core.sock").exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                assert not (core / "core.sock").exists(), f"Core shutdown unconfirmed; preserved {private}"
    if result:
        shutil.rmtree(private)
        print(json.dumps({**result, "private_core_stopped": True}))


if __name__ == "__main__":
    main()
