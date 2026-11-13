#!/usr/bin/env python3
"""Isolated real default-TUI shared Workspace/Tab/CAS lifecycle gate."""
import copy
import importlib.util
import json
import os
import re
from pathlib import Path
import select
import struct
import subprocess
import tempfile
import time
import tomllib

SPEC = importlib.util.spec_from_file_location("existing_ui_smoke", Path(__file__).with_name("existing-ui-smoke.py"))
UI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(UI)
ORACLE = Path(os.environ.get("CORE_UI_ORACLE", Path(__file__).parent / "target/debug/screen-oracle")).resolve()
DOCUMENT = "herdr-session-v1"


class Rpc:
    """An independent Core document writer, never a second interactive viewer."""
    def __init__(self, env, existing=True):
        UI.private_socket(env)
        self.child = subprocess.Popen([str(UI.HC), "connect"]+(["--existing"] if existing else []), env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.number = 0
        self.child.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 0, "method": "hello", "params": {"generation": 1, "framed": True}}).encode() + b"\n")
        self.child.stdin.flush()
        line = bytearray()
        while not line.endswith(b"\n"):
            line.extend(self.read(1))
            assert len(line) <= 65536
        assert json.loads(line)["result"]["framed"] is True

    def read(self, length):
        result = bytearray()
        deadline = time.monotonic() + 10
        while len(result) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.child.stdout], [], [], remaining)[0]:
                raise TimeoutError("Core document RPC timed out")
            data = os.read(self.child.stdout.fileno(), length - len(result))
            if not data:
                raise EOFError("Core document RPC closed")
            result.extend(data)
        return bytes(result)

    def request(self, method, params):
        self.number += 1
        payload = json.dumps({"jsonrpc": "2.0", "id": self.number, "method": method, "params": params}).encode() + b"\n"
        self.child.stdin.write(struct.pack("<IBI", 0, 0, len(payload)) + payload)
        self.child.stdin.flush()
        channel, kind, length = struct.unpack("<IBI", self.read(9))
        assert channel == 0 and kind == 0 and length <= 1024 * 1024
        reply = json.loads(self.read(length))
        assert reply["id"] == self.number
        return reply

    def doc(self):
        reply = self.request("doc.get", {"name": DOCUMENT})
        assert "error" not in reply, reply
        return reply["result"]

    def close(self):
        self.child.stdin.close()
        try:
            self.child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.child.terminate()
            self.child.wait(timeout=5)
        self.child.stdout.close()
        self.child.stderr.close()


def leaves(node):
    if node["type"] == "pane":
        return [node["terminal"]]
    return leaves(node["first"]) + leaves(node["second"])


def main():
    with tempfile.TemporaryDirectory(prefix="hc-organization-ui-", dir="/var/tmp") as scratch:
        project = Path(scratch) / "project"
        project.mkdir()
        os.chdir(project)
        home = Path(scratch)
        core = home / "core"
        env = UI.isolated_env(home, core)
        env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color", HISTFILE=str(home / "history"), PS1="org$ ",
                   CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(UI.HC), HERDR_CORE_CLIENT_ID="organization-smoke", HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12],
                   XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(home / "state"), XDG_CONFIG_HOME=str(home / "config"),
                   HERDR_CONFIG_PATH=str(home / "config.toml"), HERDR_SOCKET_PATH=str(home / "forbidden-old.sock"),
                   HERDR_CLIENT_SOCKET_PATH=str(home / "forbidden-client.sock"))
        (home / "config.toml").write_text('''[experimental]
allow_nested = true
[ui]
confirm_close = false
prompt_new_tab_name = false
prompt_new_workspace_name = false
[keys]
settings = "f1"
split_vertical = "f2"
new_tab = "f3"
close_tab = "f4"
next_tab = "f5"
next_workspace = "f6"
new_workspace = "f7"
rename_workspace = "f8"
rename_tab = "f9"
detach = "f10"
close_workspace = "f11"
close_pane = "f12"
''')
        config = core / "config/herdr-core"
        config.mkdir(parents=True, mode=0o700)
        (config / "config.toml").write_text('shell = ["/bin/bash", "--noprofile", "--norc"]\nnotes = false\n')
        assert ORACLE.is_file(), "run just core-ui-oracle"
        viewer, rpc = None, None

        def screen():
            return subprocess.run([str(ORACLE), "100", "28"], input=viewer.read(), capture_output=True, timeout=5, check=True).stdout.decode(errors="replace")

        def click_label(label):
            def locate():
                for y, line in enumerate(screen().splitlines()):
                    if label in line:
                        return line.index(label), y
                return None
            x, y = UI.until(locate, f"overlay label {label!r}")
            viewer.send(f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode())

        def check_workspace_label(label):
            before = rpc.doc()
            # The desktop selector reuses the mobile renderer; no persistent sidebar.
            viewer.send(b"\x02w")
            UI.until(lambda: screen().splitlines()[0].strip().startswith("switch"), "workspace selector")
            def named_workspace():
                text = screen()
                return text if re.search(r"· " + re.escape(label) + r"\s*$", text, re.MULTILINE) else None
            text = UI.until(named_workspace, f"workspace label {label!r}")
            viewer.send(b"\x1b")
            UI.until(lambda: not screen().splitlines()[0].strip().startswith("switch"), "return from selector")
            assert rpc.doc() == before, "local selector must not write the shared document"
            return text

        def hc(*args):
            result = subprocess.run([str(UI.HC), "--json", *args], env=env, capture_output=True, text=True, timeout=15, check=True)
            return json.loads(result.stdout)

        def send_marker(marker):
            viewer.send(f"printf '{marker[:4]}%s\\n' '{marker[4:]}'\r".encode())
            UI.until(lambda: marker in screen(), marker)

        try:
            viewer = UI.Viewer(env)
            UI.until(lambda: "org$" in screen(), "default TUI")
            rpc = Rpc(env)
            first = rpc.doc()
            assert len(first["value"]["workspaces"]) == 1
            send_marker("ORIGINAL_FIRST")
            first_address = leaves(first["value"]["workspaces"][0]["tabs"][0]["root"])[0]
            first_pid = hc("get", first_address)["pid"]
            # Settings are client-local: preview/cancel must not save, apply must
            # use the actual painted hits and write only this gate's config.
            config_path = home / "config.toml"
            original_config = config_path.read_bytes()
            viewer.send(b"\x1bOP")  # F1
            UI.until(lambda: "↑↓ select  tab section" in screen(), "settings overlay")
            click_label("nord")
            UI.until(lambda: "▸ nord" in screen(), "theme preview selection")
            click_label("esc close")
            UI.until(lambda: "↑↓ select  tab section" not in screen(), "cancel preview")
            assert config_path.read_bytes() == original_config
            viewer.send(b"\x1bOP")
            UI.until(lambda: "↑↓ select  tab section" in screen(), "settings reopen")
            click_label("nord")
            UI.until(lambda: "▸ nord" in screen(), "selected theme before apply")
            click_label("↵ apply")
            UI.until(lambda: "↑↓ select  tab section" not in screen(), "theme apply closes")
            assert tomllib.loads(config_path.read_text())["theme"]["name"] == "nord"
            viewer.send(b"\x1bOP")
            UI.until(lambda: "▸ nord ✓" in screen(), "saved theme is current")
            click_label(" sound ")
            UI.until(lambda: "sound alerts" in screen(), "sound section tab")
            click_label(" off")
            UI.until(lambda: "▸ off" in screen(), "sound choice")
            click_label("↵ apply")
            UI.until(lambda: tomllib.loads(config_path.read_text()).get("ui", {}).get("sound", {}).get("enabled") is False, "sound apply")
            UI.until(lambda: "sound alerts" in screen(), "non-theme apply keeps settings open")
            viewer.send(b"\x1b")
            UI.until(lambda: "↑↓ select  tab section" not in screen(), "settings escape")
            viewer.send(b"\x02?")  # Default prefix+? opens help.
            UI.until(lambda: "search / · scroll" in screen(), "help overlay")
            viewer.send(b"/reload")
            UI.until(lambda: "reload config" in screen() and "esc back" in screen(), "help filter")
            click_label("esc back")
            UI.until(lambda: "press / to filter by command or shortcut" in screen(), "help back clears search")
            click_label("esc close")
            UI.until(lambda: "search / · scroll" not in screen() and "keybinds" not in screen(), "help close")
            assert rpc.doc() == first, "settings/help must not write shared organization"
            assert hc("get", first_address)["pid"] == first_pid
            viewer.send(b"\x1bOR")  # F3
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"][0]["tabs"]) == 2, "tab creation")
            # Document publication does not prove the client applied its new
            # focus/surface. Wait for the actual target before sending input;
            # never retry uncertain input or redirect it to the new pane.
            UI.until(lambda: "ORIGINAL_FIRST" not in screen() and "org$" in screen(), "new tab surface is active")
            send_marker("SECOND_TAB_READY")
            viewer.send(b"\x1bOQ")  # F2
            UI.until(lambda: len(leaves(rpc.doc()["value"]["workspaces"][0]["tabs"][1]["root"])) == 2, "tab-local split")
            send_marker("SPLIT_READY")
            viewer.send(b"\x1b[24~")  # F12
            UI.until(lambda: len(rpc.doc()["value"]["retiring"]) == 1, "pane retirement")
            version = rpc.doc()["version"]
            viewer.send(b"\x1b[15~")  # F5
            UI.until(lambda: "ORIGINAL_FIRST" in screen(), "tab focus restores first program")
            assert rpc.doc()["version"] == version, "focus must not change shared document"
            viewer.send(b"\x1b[18~")  # F7
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"]) == 2, "workspace creation")
            UI.until(lambda: "ORIGINAL_FIRST" not in screen() and "org$" in screen(), "new workspace surface is active")
            send_marker("SECOND_WORKSPACE_READY")
            # Default workspace labels still come from the root pane cwd.
            labels = check_workspace_label("project")
            assert "Workspace 2" not in labels, labels
            viewer.send(b"\x1b[20~")  # F9
            UI.until(lambda: "rename tab" in screen(), "existing tab rename overlay")
            viewer.send(b"\x03SECOND_WS_TAB\r")
            UI.until(lambda: rpc.doc()["value"]["workspaces"][1]["tabs"][0]["label"] == "SECOND_WS_TAB", "tab rename")
            UI.until(lambda: "SECOND_WS_TAB" in screen() and "rename tab" not in screen(), "tab rename response closes overlay")
            viewer.send(b"\x1b[19~")  # F8
            UI.until(lambda: "rename workspace" in screen(), "existing workspace rename overlay")
            viewer.send(b"\x03SECOND_WS\r")
            UI.until(lambda: rpc.doc()["value"]["workspaces"][1]["label"] == "SECOND_WS", "workspace rename")
            UI.until(lambda: "rename workspace" not in screen(), "workspace rename response closes overlay")
            check_workspace_label("SECOND_WS")
            before = rpc.doc()
            external = copy.deepcopy(before["value"])
            external["workspaces"][0]["label"] = "EXTERNAL_WS"
            written = rpc.request("doc.set", {"name": DOCUMENT, "version": before["version"], "value": external})
            assert "error" not in written, written
            check_workspace_label("EXTERNAL_WS")
            conflict = rpc.request("doc.set", {"name": DOCUMENT, "version": before["version"], "value": before["value"]})
            assert conflict["error"]["data"]["kind"] == "conflict", conflict
            assert rpc.doc()["value"] == external
            programs = {address: hc("get", address)["pid"] for workspace in external["workspaces"] for tab in workspace["tabs"] for address in leaves(tab["root"])}
            viewer.detach()
            (core / "clients/organization-smoke/preferences.json").unlink()
            (core / "layout.json").write_text('{"must_not_be_read":"shared document is authoritative"}')
            viewer = UI.Viewer(env)
            UI.until(lambda: "ORIGINAL_FIRST" in screen(), "reattach using Core document, not local layout")
            assert hc("get", first_address)["pid"] == first_pid
            assert all(hc("get", address)["pid"] == pid for address, pid in programs.items())
            viewer.send(b"\x1bOS")  # F4
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"][0]["tabs"]) == 1, "tab close")
            viewer.send(b"\x1b[23~")  # F11
            UI.until(lambda: len(rpc.doc()["value"]["workspaces"]) == 1, "workspace close")
            UI.until(lambda: "SECOND_WORKSPACE_READY" in screen(), "focus repair to surviving workspace")
            # Positional numbering is an API fact pinned by unit tests;
            # the selector displays workspace labels.
            final = rpc.doc()["value"]
            assert final["workspaces"][0]["label"] == "SECOND_WS"
            assert len(final["retiring"]) == 3
            UI.until(lambda: all(t["state"] != "live" or t["address"] not in final["retiring"] for t in hc("ls")["terminals"]), "committed close intents terminate programs")
            assert not (home / "forbidden-old.sock").exists()
            assert not (home / "forbidden-client.sock").exists()
            viewer.detach()
            print(json.dumps({"default_tui": True, "workspaces_and_tabs": True, "tab_local_bsp": True,
                              "rename_overlays": True, "settings_preview_cancel": True,
                              "settings_mouse_apply_and_tabs": True, "settings_are_client_local": True,
                              "help_filter_header_back_and_close": True, "desktop_workspace_selector": True,
                              "focus_is_client_local": True, "external_cas_invalidation": True,
                              "stale_cas_rejected": True, "close_intent_before_termination": True,
                              "core_document_authoritative_on_reattach": True, "same_program_pids": True,
                              "full_migration_acceptance": False}))
        except Exception:
            if viewer is not None:
                print("TUI screen:", screen())
                print("Preview guard notice observed:", b"Select an available workspace" in viewer.read())
            raise
        finally:
            if viewer is not None:
                viewer.cleanup()
            if rpc is not None:
                rpc.close()
            if UI.private_socket(env).exists():
                UI.stop_private_core(env)


if __name__ == "__main__":
    main()
