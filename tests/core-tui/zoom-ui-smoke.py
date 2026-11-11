#!/usr/bin/env python3
"""Original zoom keys through the real launcher, two clients, private Core only."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("zoom_organization", HERE / "organization-ui-smoke.py")
ORG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ORG)
UI = ORG.UI
UI.HERDR = HERE.parents[1] / "scripts/core-tui/run.sh"


def main():
    home = Path(tempfile.mkdtemp(prefix="hc-zoom-ui-", dir="/var/tmp"))
    # The workspace name comes from the root pane cwd, so use a fixed directory.
    project = home / "project"
    project.mkdir()
    os.chdir(project)
    core = home / "core"
    config = core / "config/herdr-core"
    config.mkdir(parents=True, mode=0o700)
    (config / "config.toml").write_text('shell = ["/bin/bash", "--noprofile", "--norc"]\nnotes = false\n')
    trial = home / "ui.toml"
    trial.write_text('''onboarding = false
[experimental]
allow_nested = true
[ui]
sidebar_start_collapsed = true
sidebar_collapsed_mode = "hidden"
pane_borders = "off"
confirm_close = false
[keys]
split_vertical = "f2"
split_horizontal = "f3"
close_pane = "f4"
cycle_pane_next = "f5"
zoom = "f6"
focus_pane_left = "f7"
focus_pane_right = "f8"
swap_pane_left = "f9"
swap_pane_right = "f11"
detach = "f10"
''')
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_", "CORE_UI_")) and k not in {
        "DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR", "TMUX", "STY", "SSH_CONNECTION", "SSH_TTY", "VSCODE_IPC_HOOK_CLI"}}
    env.setdefault("CARGO_HOME", str(Path.home() / ".cargo"))
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color", HISTFILE=str(home / "history"),
               CORE_TUI_HC=str(UI.HC), CORE_UI_HOME=str(core), CORE_UI_CONFIG=str(trial),
               HC_SESSION="herdr-tui", HC_SOCKET=str(core / "core.sock"), XDG_RUNTIME_DIR=str(core / "run"),
               XDG_STATE_HOME=str(core / "state"), XDG_CONFIG_HOME=str(core / "config"))
    viewers, rpc, result = [], None, None

    def viewer(identity):
        result = UI.Viewer(dict(env, CORE_UI_CLIENT_ID=identity))
        viewers.append(result)
        return result

    def screen(v):
        for other in viewers:
            if other.pid is not None:
                other.read()  # Keep every owned PTY draining, including the passive client.
        return subprocess.run([str(ORG.ORACLE), "100", "28", json.dumps(v.resizes)], input=v.read(),
                              capture_output=True, timeout=5, check=True).stdout.decode(errors="replace")

    def hc(*args):
        for v in viewers:
            if v.pid is not None:
                v.read()
        result = subprocess.run([str(UI.HC), "--json", *args], env=env, capture_output=True, text=True, timeout=15, check=True)
        return json.loads(result.stdout)

    def grid(address):
        facts = hc("get", address)
        return facts["cols"], facts["rows"]

    def mark(v, name):
        v.send(f"stty -echo; printf '\\033[2J\\033[3J\\033[HZOOM_%s\\n' '{name}'\r".encode())
        UI.until(lambda: f"ZOOM_{name}" in screen(v), f"executed marker {name}")

    try:
        a = viewer("zoom-a")
        # This fixture collapses the sidebar, so the workspace name is not on
        # screen. Wait for the client's real first paint instead: a live Core
        # program plus a rendered frame.
        terminals = UI.until(lambda: [t for t in hc("ls")["terminals"] if t["state"] == "live"], "private Core program startup")
        UI.until(lambda: screen(a).strip(), "first original TUI")
        root = terminals[0]["address"]
        assert hc("get", root)["labels"].get("herdr") == "tui"
        root_pid = hc("get", root)["pid"]
        mark(a, "ROOT")
        rpc = ORG.Rpc(env)
        version = rpc.doc()["version"]
        full = grid(root)
        a.send(b"\x1b[17~")  # F6: original single-pane no-op.
        a.send(b"printf 'SINGLE_%s\\n' OK\r")
        UI.until(lambda: "SINGLE_OK" in screen(a), "single-pane zoom remains usable")
        assert grid(root) == full and rpc.doc()["version"] == version
        a.send(b"\x1bOQ")  # F2: original vertical split.
        UI.until(lambda: len(ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"])) == 2, "shared split")
        right = next(t["address"] for t in hc("ls")["terminals"] if t["address"] != root)
        mark(a, "RIGHT")
        right_pid = hc("get", right)["pid"]
        UI.until(lambda: grid(root)[0] < full[0] and grid(right)[0] < full[0], "split geometry committed")
        split, left_split = grid(right), grid(root)
        version = rpc.doc()["version"]
        b = viewer("zoom-b")
        UI.until(lambda: "ZOOM_ROOT" in screen(b) and "ZOOM_RIGHT" in screen(b), "second client remains split")
        a.send(b"\x1b[17~")
        UI.until(lambda: grid(right) == full and "ZOOM_ROOT" not in screen(a) and "ZOOM_RIGHT" in screen(a), "zoom uses whole original area")
        assert "ZOOM_ROOT" in screen(b) and "ZOOM_RIGHT" in screen(b), "zoom is client-local"
        assert grid(root) == left_split, "hidden root must not receive the zoomed program's geometry"
        assert rpc.doc()["version"] == version
        # B zooms the pane hidden in A, changing its authoritative Core grid.
        b.send(b"\x1b[17~")
        UI.until(lambda: grid(root) == full and "ZOOM_RIGHT" not in screen(b), "other client zooms A's hidden pane")
        a.send(b"\x1b[17~")
        UI.until(lambda: grid(root) == left_split and grid(right) == split and "ZOOM_ROOT" in screen(a), "revealed pane reannounces unchanged desired grid after foreign resize")
        assert "ZOOM_RIGHT" not in screen(b), "A unzoom must not clear B's zoom preference"
        b.send(b"\x1b[17~")
        UI.until(lambda: "ZOOM_RIGHT" in screen(b), "other client independently unzooms")
        a.send(b"\x1b[17~")
        UI.until(lambda: grid(right) == full and "ZOOM_ROOT" not in screen(a), "return to zoom before navigation")
        # Original swaps edit the hidden BSP, preserve zoom, and follow the source.
        a.send(b"\x1b[20~")
        UI.until(lambda: ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"]) == [right, root], "swap source to the left while zoomed")
        UI.until(lambda: "ZOOM_RIGHT" in screen(a) and "ZOOM_ROOT" not in screen(a), "swap follows source identity without unzooming")
        a.send(b"\x1b[23~")
        UI.until(lambda: ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"]) == [root, right], "swap back through original key")
        version += 2
        a.send(b"\x1b[23~")  # No right neighbor: must not publish an identical BSP.
        a.send(b"printf 'SWAP_NOOP_%s\\n' OK\r")
        UI.until(lambda: "SWAP_NOOP_OK" in screen(a), "ordered input barrier after no-neighbor swap")
        assert rpc.doc()["version"] == version
        a.send(b"\x1b[18~")  # F7: original navigation uses underlying BSP while zoomed.
        UI.until(lambda: grid(root) == full and "ZOOM_ROOT" in screen(a) and "ZOOM_RIGHT" not in screen(a), "navigate left while zoomed")
        a.send(b"\x1b[19~")
        UI.until(lambda: "ZOOM_ROOT" not in screen(a) and "ZOOM_RIGHT" in screen(a), "navigate right stays zoomed")
        a.send(b"\x1b[17~")
        UI.until(lambda: grid(root) == left_split and grid(right) == split and "ZOOM_ROOT" in screen(a), "unzoom restores both BSP geometries")
        a.send(b"\x1b[17~")
        UI.until(lambda: grid(right) == full, "re-zoom before client replacement")
        a.detach()
        UI.until(lambda: grid(right) == split, "remaining split client owns grid after detach")
        a = viewer("zoom-a")
        UI.until(lambda: grid(right) == full and "ZOOM_RIGHT" in screen(a) and "ZOOM_ROOT" not in screen(a), "client-local zoom survives ordinary detach/reattach")
        assert "ZOOM_ROOT" in screen(b) and "ZOOM_RIGHT" in screen(b)
        assert hc("get", root)["pid"] == root_pid and hc("get", right)["pid"] == right_pid
        assert rpc.doc()["version"] == version
        a.send(b"\x1bOR")  # F3: splitting resets zoom, as native Tab did.
        UI.until(lambda: len(ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"])) == 3, "split while zoomed")
        mark(a, "THIRD")
        UI.until(lambda: "ZOOM_ROOT" in screen(a) and "ZOOM_RIGHT" in screen(a), "split clears zoom")
        a.send(b"\x1b[17~")
        UI.until(lambda: "ZOOM_ROOT" not in screen(a) and "ZOOM_RIGHT" not in screen(a), "zoom new pane")
        b.send(b"\x1bOS")  # F4: remote root removal must also clear A's zoom.
        UI.until(lambda: len(ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][0]["root"])) == 2, "remote close commits")
        UI.until(lambda: "ZOOM_RIGHT" in screen(a) and "ZOOM_THIRD" in screen(a), "remote membership removal restores BSP")
        a.send(b"\x1b[17~")
        UI.until(lambda: "ZOOM_RIGHT" not in screen(a), "zoom before local close")
        a.send(b"\x1bOS")
        UI.until(lambda: "ZOOM_RIGHT" in screen(a), "closing zoomed pane restores survivor")
        assert hc("get", right)["pid"] == right_pid
        a.send(b"exit\r")
        a.wait_exit()
        b.wait_exit()
        assert not rpc.doc()["value"]["workspaces"]
        for v in viewers:
            assert b"not implemented" not in v.output and b"Input rejected" not in v.output
        result = {"actual_launcher": True, "original_zoom_keys_and_geometry": True,
                          "single_pane_noop": True, "navigate_while_zoomed": True, "client_local_zoom": True,
                          "original_swap_keys_preserve_zoom_and_focus": True, "no_neighbor_swap_does_not_publish": True,
                          "local_zoom_preferences_reattach": True, "split_and_local_remote_close_clear_zoom": True,
                          "zoom_does_not_write_shared_bsp": True, "surviving_program_pid_unchanged": True,
                          "ordinary_reattach_is_not_server_handoff": True, "full_migration_acceptance": False}
    except Exception:
        for index, v in enumerate(viewers):
            if v.pid is not None:
                print(f"TUI {index}:\n{screen(v)}")
        print(f"failed zoom fixture retained: {home}")
        raise
    finally:
        try:
            for v in viewers:
                v.cleanup()
        finally:
            try:
                if rpc is not None:
                    rpc.close()
            finally:
                if (core / "core.sock").exists():
                    subprocess.run([str(UI.HC), "server", "stop"], env=env, capture_output=True, timeout=15, check=True)
                    UI.until(lambda: not (core / "core.sock").exists(), "private Core stop")
        if result is not None:
            shutil.rmtree(home)
    print(json.dumps(dict(result, private_core_stopped=True)))


if __name__ == "__main__":
    main()
