#!/usr/bin/env python3
"""Exercise ht against a private Core with the desktop App's document format.

The writer is a fixture, not the GUI. It uses the mirror.rs/layout.rs schema
from herdr-app 0ac92e4, including independent node and terminal identities.
"""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile

import harness as UI

ORG = runpy.run_path(str(Path(__file__).with_name("organization-ui-smoke.py")))
DOC = "herdr-app"


def block(node_id, **content):
    return {"id": node_id, "type": "block", "content": content}


def main():
    with tempfile.TemporaryDirectory(prefix="hc-app-layout-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        env = UI.isolated_env(home, home / "core")
        # Exercise the default socket and layout on a gate-owned runtime.
        env["HC_SESSION"] = "default"
        Path(env["HERDR_CONFIG_PATH"]).write_text('''[terminal]
default_shell = "/bin/sh"
[keys]
detach = "f10"
close_workspace = "f11"
''')
        rpc = wide = narrow = None

        def call(method, **params):
            reply = rpc.request(method, params)
            assert "error" not in reply, reply
            return reply["result"]

        def run(marker):
            return call("terminal.run", argv=["/bin/sh", "-c", f"printf '{marker}\\n'; exec /bin/sh"],
                        cwd=str(home), no_parent=True, labels={"herdr.space": "space-main"},
                        size={"cols": 80, "rows": 24})

        def publish(spaces):
            current = rpc.doc(DOC)
            call("doc.set", name=DOC, version=current["version"], value={"spaces": spaces})
            return rpc.doc(DOC)

        def screen(peer):
            _, cols, rows = peer.resizes[0]
            oracle = subprocess.run([str(ORG["ORACLE"]), str(cols), str(rows)], input=peer.read(),
                                    capture_output=True, timeout=5, check=True)
            return oracle.stdout.decode(errors="replace")

        def tab_bar(peer):
            return screen(peer).splitlines()[-1]

        def showing(peer, expected, *excluded):
            return UI.until(lambda: (s := screen(peer)) and expected in s and not any(e in s for e in excluded),
                            f"showing {expected}, not {excluded}")

        def click(peer, label, row):
            def locate():
                lines = screen(peer).splitlines()
                return len(lines) > row and lines[row].find(label) + 1

            x = UI.until(locate, f"tab entry {label}")
            peer.send(f"\x1b[<0;{x};{row + 1}M\x1b[<0;{x};{row + 1}m".encode())

        try:
            wide = UI.Viewer(env, shared=True)
            wide.wait_exit(success=False)
            socket = UI.private_socket(env, shared=True)
            assert not socket.exists(), "read-only launch must not start Core"
            assert b"check that HC_SOCKET points to a running Core" in wide.output, wide.output
            assert bytes(socket) in wide.output, wide.output
            rpc = ORG["Rpc"](env, existing=False, shared=True)
            absent = rpc.doc(DOC)
            assert absent["value"] is None
            a, b, c = [run(f"APP_SCREEN_{letter}") for letter in "ABC"]
            aa, bb, cc = (r["address"] for r in (a, b, c))
            first = {"id": "tab-pair", "name": "Pair", "root": {
                "id": "split-node", "type": "row", "children": [
                    dict(block("node-a", terminal=aa), size={"weight": 3.0}),
                    block("plugin", mod="example", props={"preserved": True}),
                    {"id": "nested-stack", "type": "stack", "active": "plugin-b", "children": [
                        block("plugin-b", mod="other"), block("node-b", terminal=bb),
                    ]},
                ],
            }}
            second = {"id": "tab-single", "root": dict(block("node-c", terminal=cc), closable=False)}
            plugin = {"id": "tab-plugin-only", "root": block("mod-only", mod="other")}

            def space(*tabs):
                return {"id": "space-main", "name": "App Space", "folder": str(home),
                        "tabs": tabs, "future_field": {"preserved": 1}}

            wide_env = dict(env, HERDR_CORE_CLIENT_ID="app-wide")
            wide = UI.Viewer(wide_env, shared=True)
            # Without an App document the shared viewer waits, even with live Core terminals.
            UI.until(lambda: b"\x1b[?1049h" in wide.read(), "App viewer starts without a layout")
            assert rpc.doc(DOC) == absent
            draft = publish([space(first, plugin, second)])
            showing(wide, "APP_SCREEN_A", "APP_SCREEN_B", "APP_SCREEN_C")
            heading = tab_bar(wide)
            assert "1.1" in heading and "1.2" in heading
            for key, target in [(b"n", "B"), (b"n", "C"), (b"n", "A"), (b"p", "C"), (b"p", "B")]:
                wide.send(b"\x01" + key)
                showing(wide, f"APP_SCREEN_{target}")
            wide.send(b"\x012")
            showing(wide, "APP_SCREEN_C", "APP_SCREEN_A", "APP_SCREEN_B")
            wide.send(b"\x01x")  # The App marks this terminal as not closable.
            showing(wide, "not closable")
            assert call("terminal.get", terminal=cc)["pid"] == c["pid"]
            wide.send(b"\x011")
            showing(wide, "APP_SCREEN_B", "APP_SCREEN_A", "APP_SCREEN_C")
            wide.send(b"\x01w\t")  # Pane cycling is local to the workspace selector.
            showing(wide, "APP_SCREEN_A", "APP_SCREEN_B", "APP_SCREEN_C")
            wide.send(b"\x01w\x1b[Z")
            showing(wide, "APP_SCREEN_B", "APP_SCREEN_A", "APP_SCREEN_C")
            wide.send(b"printf 'FROM_%s_HERE\\n' 'HT'\r")
            showing(wide, "FROM_HT_HERE")
            assert "FROM_HT_HERE" in call("terminal.read", terminal=bb)["text"]

            narrow = UI.Viewer(dict(env, HERDR_CORE_CLIENT_ID="app-narrow"), cols=48, rows=22, shared=True)
            click(narrow, "1.1", 1)
            showing(narrow, "APP_SCREEN_A", "APP_SCREEN_B", "APP_SCREEN_C")
            UI.until(lambda: call("terminal.get", terminal=aa)["cols"] == 48, "selected pane has mobile grid")
            assert call("terminal.get", terminal=bb)["cols"] == 100
            showing(wide, "APP_SCREEN_B", "APP_SCREEN_A", "APP_SCREEN_C")

            # An external reorder must keep the selected PTY, not the old
            # numeric index.
            wide.send(b"printf 'STILL_%s_LIVE\\n' 'APP'\r")
            showing(wide, "STILL_APP_LIVE")
            assert rpc.doc(DOC) == draft
            moved = publish([space(second, first, plugin)])
            UI.until(lambda: "2.2" in tab_bar(wide), "App reorders tabs")
            showing(wide, "APP_SCREEN_B", "APP_SCREEN_A", "APP_SCREEN_C")
            showing(narrow, "APP_SCREEN_A", "APP_SCREEN_B", "APP_SCREEN_C")
            assert rpc.doc(DOC) == moved
            wide.detach()
            wide = UI.Viewer(wide_env, shared=True)
            showing(wide, "APP_SCREEN_B", "APP_SCREEN_A", "APP_SCREEN_C")

            emptied = publish([])
            UI.until(lambda: "APP_SCREEN_B" not in screen(wide) and "2.2" not in screen(wide), "App clears layout")
            restored = publish([space(second, first, plugin)])
            showing(wide, "APP_SCREEN_C", "APP_SCREEN_A", "APP_SCREEN_B")
            assert emptied["value"] == {"spaces": []}
            assert rpc.doc(DOC) == restored
            pid_by_address = {r["address"]: r["pid"] for r in (a, b, c)}
            actual = subprocess.run([str(UI.HC), "--json", "ls", "--all"], env=env, capture_output=True,
                                    text=True, timeout=10, check=True)
            records = json.loads(actual.stdout)["terminals"]
            assert {r["address"] for r in records} == set(pid_by_address), "no extra program was created"
            for address, pid in pid_by_address.items():
                record = call("terminal.get", terminal=address)
                assert record["state"] == "live" and record["pid"] == pid
            wide.send(b"\x012")
            showing(wide, "APP_SCREEN_A", "APP_SCREEN_B")
            wide.send(b"\x01n")
            showing(wide, "APP_SCREEN_B")
            wide.send(b"\x01x")
            showing(wide, "APP_SCREEN_A", "APP_SCREEN_B")
            UI.until(lambda: "2.2" not in screen(narrow).splitlines()[1], "other viewer drops the ended pane")
            closed = call("terminal.get", terminal=bb)
            assert closed["state"] == "finished"
            for survivor in (aa, cc):
                assert call("terminal.get", terminal=survivor)["pid"] == pid_by_address[survivor]
            assert rpc.doc(DOC) == restored
            wide.detach()
            wide = UI.Viewer(wide_env, shared=True)
            showing(wide, "APP_SCREEN_A", "APP_SCREEN_B")
            assert "2.2" not in tab_bar(wide)

            wide.send(b"\x01c")
            def appended():
                current = rpc.doc(DOC)
                return current if current["version"] > restored["version"] else None
            created = UI.until(appended, "prefix+c writes a new App tab")
            assert created["version"] == restored["version"] + 1
            spaces = created["value"]["spaces"]
            tab = spaces[0]["tabs"].pop()
            assert created["value"] == restored["value"], "only the new tab is appended"
            address = tab["root"]["children"][0]["content"]["terminal"]
            UI.until(lambda: "3" in tab_bar(wide)
                    and "APP_SCREEN_A" not in screen(wide), "new tab gets local focus")
            wide.send(b"printf 'NEW_%s_READY\\n' 'TAB'\r")
            showing(wide, "NEW_TAB_READY")
            assert "NEW_TAB_READY" in call("terminal.read", terminal=address)["text"]
            facts = call("terminal.get", terminal=address)
            assert facts["labels"]["herdr.space"] == "space-main"
            assert facts["labels"]["herdr.tab"] == tab["id"]
            assert facts["cwd"] == str(home)
            wide.send(b"printf 'PANE_TERM_%s\\n' \"$TERM\"\r")
            showing(wide, "PANE_TERM_xterm-kitty")
            showing(narrow, "APP_SCREEN_C", "NEW_TAB_READY")
            click(narrow, "3", 1)  # All three tabs now have a single pane.
            showing(narrow, "NEW_TAB_READY", "APP_SCREEN_C")

            wide.send(b"\x01T")
            showing(wide, "rename tab")
            wide.send(b"\x03Shared QA Tab\r")
            UI.until(lambda: rpc.doc(DOC)["value"]["spaces"][0]["tabs"][-1]["name"] == "Shared QA Tab",
                     "rename tab in App layout")
            showing(narrow, "Shared QA Tab")
            renamed = rpc.doc(DOC)["value"]
            original_tabs = renamed["spaces"][0]["tabs"]
            wide.send(b"\x012")
            showing(wide, "APP_SCREEN_A", "NEW_TAB_READY")
            wide.send(b"\x1b[46;7u")  # Ctrl+Alt+period: move Pair one visible slot toward the end.
            expected = [original_tabs[i] for i in (0, 2, 3, 1)]
            UI.until(lambda: rpc.doc(DOC)["value"]["spaces"][0]["tabs"] == expected,
                     "keyboard moves one whole tab, preserving the plugin tab")
            showing(wide, "APP_SCREEN_A", "NEW_TAB_READY")
            showing(narrow, "NEW_TAB_READY")

            # Drag Pair back left of Shared QA Tab; the hidden plugin keeps its data.
            def reordered_heading():
                line = tab_bar(wide)
                return line if 0 <= line.find("Shared QA Tab") < line.find("Pair") else None

            heading = UI.until(reordered_heading, "reordered tab bar is rendered")
            source = heading.index("Pair") + 1
            destination = heading.index("Shared QA Tab")  # first half of its tab
            tab_row = wide.resizes[0][2]  # The mouse report uses one-based screen coordinates.
            wide.send(f"\x1b[<0;{source};{tab_row}M\x1b[<32;{destination};{tab_row}M\x1b[<0;{destination};{tab_row}m".encode())
            expected = [original_tabs[i] for i in (0, 2, 1, 3)]
            UI.until(lambda: rpc.doc(DOC)["value"]["spaces"][0]["tabs"] == expected,
                     "drag reorders by visible tab anchors")
            existing_space = rpc.doc(DOC)["value"]["spaces"][0]
            for survivor in (aa, cc):
                assert call("terminal.get", terminal=survivor)["pid"] == pid_by_address[survivor]
            assert call("terminal.get", terminal=address)["pid"] == facts["pid"]

            wide.send(b"\x1b[23~")  # F11: explicit test-only close binding.
            showing(wide, "not closable")
            assert rpc.doc(DOC)["value"]["spaces"] == [existing_space]
            assert call("terminal.get", terminal=aa)["pid"] == pid_by_address[aa]

            wide.send(b"\x01N")
            UI.until(lambda: len(rpc.doc(DOC)["value"]["spaces"]) == 2, "prefix+Shift+n creates a Space")
            new = rpc.doc(DOC)["value"]["spaces"][1]
            new_addr = new["tabs"][0]["root"]["children"][0]["content"]["terminal"]
            UI.until(lambda: "Shared QA Tab" not in screen(wide), "new Space takes local focus")
            showing(narrow, "NEW_TAB_READY")
            wide.send(b"\x01W")
            showing(wide, "rename workspace")
            wide.send(b"\x03Workspace 2\r")
            UI.until(lambda: rpc.doc(DOC)["value"]["spaces"][1]["name"] == "Workspace 2",
                     "rename Space in the shared layout")
            UI.until(lambda: "rename workspace" not in screen(wide), "Space rename dialog closes")
            wide.send(b"\x01w")
            showing(wide, "Workspace 2")
            wide.send(b"\x1b")
            UI.until(lambda: "Workspace 2" not in screen(wide), "return from Space switcher")
            assert call("terminal.get", terminal=new_addr)["cwd"] == str(home)
            wide.send(b"\x01c")
            UI.until(lambda: len(rpc.doc(DOC)["value"]["spaces"][1]["tabs"]) == 2,
                     "second terminal in the new Space")
            new_tabs = rpc.doc(DOC)["value"]["spaces"][1]["tabs"]
            new_addrs = [t["root"]["children"][0]["content"]["terminal"] for t in new_tabs]
            wide.send(b"\x1b[23~")
            UI.until(lambda: rpc.doc(DOC)["value"]["spaces"] == [existing_space], "close only the selected Space")
            for addr in new_addrs:
                assert call("terminal.get", terminal=addr)["state"] == "finished"
            for survivor in (aa, cc):
                assert call("terminal.get", terminal=survivor)["pid"] == pid_by_address[survivor]
            showing(narrow, "NEW_TAB_READY")

            publish([])
            UI.until(lambda: "Shared QA Tab" not in screen(wide), "App layout becomes empty")
            wide.send(b"\x01c")
            UI.until(lambda: len(rpc.doc(DOC)["value"]["spaces"]) == 1, "prefix+c adds the first tab")
            wide.detach()
            narrow.detach()
            print("App-format document: tab rename/reorder, Space create/rename/close, pane cycling and independent focus")
        finally:
            for viewer in (narrow, wide):
                if viewer:
                    viewer.cleanup()
            if rpc:
                rpc.close()
            UI.stop_private_core(env, shared=True)


if __name__ == "__main__":
    main()
