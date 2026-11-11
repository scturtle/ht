#!/usr/bin/env python3
"""Two real default TUIs, one Core, different grids; no system clipboard or session."""
import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

SPEC = importlib.util.spec_from_file_location("organization_smoke", Path(__file__).with_name("organization-ui-smoke.py"))
ORG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ORG)
UI, ORACLE = ORG.UI, ORG.ORACLE


def main():
    with tempfile.TemporaryDirectory(prefix="hc-multiclient-ui-", dir="/var/tmp") as scratch:
        home, viewers = Path(scratch), []
        core = home / "core"
        env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_")) and k not in {
            "DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR", "TMUX", "STY", "SSH_CONNECTION", "SSH_TTY", "VSCODE_IPC_HOOK_CLI"}}
        env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color", HISTFILE=str(home / "history"), PS1="multi$ ",
                   CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(UI.HC), HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12],
                   XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(home / "state"), XDG_CONFIG_HOME=str(home / "config"),
                   HERDR_CONFIG_PATH=str(home / "config.toml"), HERDR_SOCKET_PATH=str(home / "forbidden-old.sock"),
                   HERDR_CLIENT_SOCKET_PATH=str(home / "forbidden-client.sock"))
        (home / "config.toml").write_text('''onboarding = false
[experimental]
allow_nested = true
[ui]
sidebar_start_collapsed = true
sidebar_collapsed_mode = "hidden"
pane_borders = "off"
confirm_close = false
prompt_new_tab_name = false
[keys]
new_tab = "f3"
close_tab = "f4"
next_tab = "f5"
copy_mode = "f7"
rename_pane = "f8"
detach = "f10"
''')
        config = core / "config/herdr-core"
        config.mkdir(parents=True, mode=0o700)
        (config / "config.toml").write_text('shell = ["/bin/bash", "--noprofile", "--norc"]\nnotes = false\n')
        address, rpc = None, None

        def hc(*args):
            result = subprocess.run([str(UI.HC), "--json", *args], env=env, capture_output=True, text=True, timeout=15, check=True)
            return json.loads(result.stdout)

        def screen(viewer, size):
            return subprocess.run([str(ORACLE), "100", "28", json.dumps(viewer.resizes)], input=viewer.read(), capture_output=True, timeout=5, check=True).stdout.decode(errors="replace")

        def grid():
            facts = hc("get", address)
            return facts["cols"], facts["rows"]

        def wrapped(viewer, size, char, marker):
            lines = screen(viewer, size).splitlines()
            prefix = char * 20
            for row, line in enumerate(lines[:-1]):
                if prefix in line:
                    column = line.index(prefix)
                    return lines[row + 1][column:].startswith(marker)
            return False

        def pattern(writer, canonical, char, marker):
            writer.send(f"printf '\\033[2J\\033[3J\\033[H%s{marker}\\r\\n' '{char * canonical[0]}'\r".encode())

        try:
            a = UI.Viewer(env); viewers.append(a)
            UI.until(lambda: "multi$" in screen(a, (100,28)), "first default TUI program prompt")
            address = hc("ls")["terminals"][0]["address"]
            pid = hc("get", address)["pid"]
            a.send(b"stty -echo; printf 'MULTI_%s\\n' READY\r")
            UI.until(lambda: "MULTI_READY" in screen(a,(100,28)), "echo disabled in shared program")
            a_grid = grid()
            rpc = ORG.Rpc(env)
            version = rpc.doc()["version"]
            b = UI.Viewer(env, cols=72, rows=19); viewers.append(b)
            UI.until(lambda: "MULTI_READY" in screen(b,(72,19)), "second simultaneous default TUI")
            UI.until(lambda: grid()[0] < a_grid[0] and grid()[1] < a_grid[1], "smaller interactive viewer leads")
            b_grid = grid()
            assert len([t for t in hc("ls")["terminals"] if t["state"] == "live"]) == 1
            assert not (core / "client.lock").exists()
            assert len(list((core / "clients").iterdir())) == 2, "distinct automatic host-TTY presentation identities"
            # Reuse the actual original naming dialog; shared label lives only in Core.
            label_key, pane_name = "herdr.display_name", "研发构建"
            a.send(b"\x1b[19~")
            UI.until(lambda: "rename pane" in screen(a,(100,28)), "original pane rename dialog")
            a.send(("  " + pane_name + "  \r").encode())
            UI.until(lambda: hc("get",address)["labels"].get(label_key) == pane_name, "pane name persisted in Core")
            UI.until(lambda: "rename pane" not in screen(a,(100,28)), "pane rename response")
            b.send(b"\x1b[19~")
            UI.until(lambda: "rename pane" in screen(b,(72,19)) and pane_name in screen(b,(72,19)), "second client receives shared name")
            b.send(b"\r")
            UI.until(lambda: "rename pane" not in screen(b,(72,19)), "second client original submit")
            assert grid() == b_grid and rpc.doc()["version"] == version, "metadata must not steal geometry or write BSP document"
            assert hc("get",address)["pid"] == pid and hc("get",address).get("status") is None
            pattern(b,b_grid,"B","WRAP_B")
            UI.until(lambda: wrapped(b,(72,19),"B","WRAP_B"), "leader wraps at its Core grid")
            UI.until(lambda: wrapped(a,(100,28),"B","WRAP_B"), "larger client parser follows smaller authoritative grid")
            # A single harmless key transfers leadership before submitting a
            # multi-key command. Do not retry a partially cancelled command.
            a.send(b"\x1b[C")
            UI.until(lambda: grid() == a_grid, "human input transfers leadership to larger client")
            UI.until(lambda: "B" * 20 + "WRAP_B" in screen(a,(100,28)),
                     "larger client consumes completed reflow, not just Core geometry")
            pattern(a,a_grid,"A","WRAP_A")
            UI.until(lambda: wrapped(a,(100,28),"A","WRAP_A"), "larger leader wraps at restored grid")
            UI.until(lambda: wrapped(b,(72,19),"A","WRAP_A"), "smaller client parser follows larger grid, clipping only presentation")
            for _ in range(5):
                assert grid() == a_grid, "no resize feedback/leadership fight"
            b.resize(90,25)
            UI.until(lambda: grid()[0] <= 90 and grid()[1] <= 25 and grid() != a_grid, "explicit window resize claims leadership")
            b_small = grid()
            UI.until(lambda: "A" * (a_grid[0] - b_small[0]) + "WRAP_A" in screen(b,(90,25)),
                     "resized client consumes completed narrower reflow")
            pattern(b,b_small,"C","WRAP_RESIZED")
            UI.until(lambda: wrapped(a,(100,28),"C","WRAP_RESIZED"), "authoritative resize precedes live bytes for other client")
            UI.until(lambda: wrapped(b,(90,25),"C","WRAP_RESIZED"), "resized viewer gets replacement paint")
            a.send(b"\x1b[18~")
            UI.until(lambda: " COPY " in screen(a,(100,28)), "copy mode with a foreign winning grid")
            a.send(b"/WRAP_RESIZED\r")
            UI.until(lambda: "1/1" in screen(a,(100,28)), "search in authoritative retained coordinates")
            a.send(b"y")
            UI.until(lambda: b"WRAP_RESIZED" in [base64.b64decode(m) for m in re.findall(rb"\x1b\]52;c;([^\x07\x1b]*)(?:\x07|\x1b\\)",a.read())], "multi-client copy via captured OSC52")
            assert grid() == b_small, "copy and automatic replies must not claim size leadership"
            assert rpc.doc()["version"] == version, "geometry/copy must not write organization"
            a.send(b"\x1bOR")  # F3: shared new tab, local focus only.
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"][0]["tabs"]) == 2, "shared tab creation")
            UI.until(lambda: re.search(r"\b2\b", screen(a,(100,28)).splitlines()[0]) and "WRAP_RESIZED" not in screen(a,(100,28)), "creator focuses new tab")
            UI.until(lambda: re.search(r"\b2\b", screen(b,(90,25)).splitlines()[0]) and "WRAP_RESIZED" in screen(b,(90,25)), "other client sees membership without losing focus")
            second_address = ORG.leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][1]["root"])[0]
            UI.until(lambda: "multi$" in screen(a,(100,28)), "new tab program prompt before input")
            a.send(b"stty -echo; printf 'NEW_TAB_%s\\n' READY\r")
            UI.until(lambda: "NEW_TAB_READY" in screen(a,(100,28)), "new program on creator")
            second = hc("get",second_address)
            assert (second["cols"],second["rows"]) == a_grid, "passive hidden attach must not steal new program geometry"
            b.resize(84,23)
            UI.until(lambda: grid()[0] <= 84 and grid()[1] <= 23, "resize visible old tab")
            second = hc("get",second_address)
            assert (second["cols"],second["rows"]) == a_grid, "background tab must not send a competing resize"
            b.send(b"\x1b[15~")  # F5: activate formerly hidden tab.
            UI.until(lambda: "NEW_TAB_READY" in screen(b,(84,23)), "client-local tab focus")
            UI.until(lambda: hc("get",second_address)["cols"] <= 84, "activating hidden tab advertises latest desired geometry")
            b.send(b"\x1b[15~")
            UI.until(lambda: "WRAP_RESIZED" in screen(b,(84,23)), "other client returns to original tab")
            a.send(b"\x1bOS")  # F4: retire shared second tab.
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"][0]["tabs"]) == 1, "shared tab close")
            UI.until(lambda: not re.search(r"\b2\b", screen(a,(100,28)).splitlines()[0]) and "C" * 20 in screen(a,(100,28)), "creator focus repairs to surviving tab (marker may reflow)")
            b.detach()
            UI.until(lambda: grid() == a_grid, "detaching leader restores remaining viewer's requested grid")
            UI.until(lambda: "C" * 20 + "WRAP_RESIZED"[:a_grid[0] - b_small[0]] in screen(a,(100,28)),
                     "survivor consumes completed restored-grid reflow")
            pattern(a,a_grid,"D","WRAP_DETACHED")
            UI.until(lambda: wrapped(a,(100,28),"D","WRAP_DETACHED"), "survivor keeps valid parser after detach")
            assert hc("get",address)["pid"] == pid
            assert rpc.doc()["version"] == version + 2, "only explicit tab creation/close write shared organization"
            assert not (home / "forbidden-old.sock").exists() and not (home / "forbidden-client.sock").exists()
            # Final shared membership disappearing must exit BOTH TUIs, not Core.
            b = UI.Viewer(env)
            viewers.append(b)
            UI.until(lambda: "WRAP_DETACHED" in screen(b,(100,28)), "second client before final exit")
            b.send(b"\x1b[19~")
            UI.until(lambda: "rename pane" in screen(b,(100,28)) and pane_name in screen(b,(100,28)), "name survives detach and reattach")
            b.send(b"\x15\r")
            UI.until(lambda: label_key not in hc("get",address)["labels"], "empty original dialog clears name")
            UI.until(lambda: "rename pane" not in screen(b,(100,28)), "clear-name response")
            a.send(b"\x1b[19~")
            UI.until(lambda: "rename pane" in screen(a,(100,28)) and pane_name not in screen(a,(100,28)), "other client receives cleared name")
            a.send(b"\r")
            UI.until(lambda: "rename pane" not in screen(a,(100,28)), "empty-name submission")
            assert rpc.doc()["version"] == version + 2
            a.send(b"exit\r")
            a.wait_exit()
            b.wait_exit()
            assert not rpc.doc()["value"]["workspaces"]
            print(json.dumps({"simultaneous_default_tuis":True,"same_core_program_pid":True,
                              "automatic_per_tty_identity":True,"ordered_authoritative_wrap":True,
                              "input_and_resize_leadership":True,"no_resize_feedback":True,
                              "detach_restores_remaining_grid":True,"copy_with_foreign_winning_grid":True,
                              "both_clients_exit_after_last_shared_pane":True,
                              "original_pane_rename_shared_persistent_and_clearable":True,
                              "shared_tabs_with_client_local_focus":True,"passive_hidden_views_do_not_steal_geometry":True,
                              "clipboard":"captured OSC52 only","geometry_does_not_write_shared_document":True,
                              "grids":[a_grid,b_grid,b_small],"full_migration_acceptance":False}))
        except Exception:
            for index, viewer in enumerate(viewers):
                if viewer.pid is not None:
                    print(f"TUI {index}:", screen(viewer,(100,28) if index == 0 else (90,25)))
            raise
        finally:
            for viewer in viewers: viewer.cleanup()
            if rpc is not None: rpc.close()
            if (core / "core.sock").exists():
                subprocess.run([str(UI.HC),"server","stop"],env=env,capture_output=True,timeout=15,check=True)


if __name__ == "__main__":
    main()
