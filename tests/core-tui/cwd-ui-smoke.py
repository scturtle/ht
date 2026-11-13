#!/usr/bin/env python3
"""Fresh attach must use the first tab's live cwd, without a follow-up action."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile

import harness as H

SPEC = importlib.util.spec_from_file_location("organization_gate", Path(__file__).with_name("organization-ui-smoke.py"))
ORG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ORG)
ORACLE = Path(os.environ.get("CORE_UI_ORACLE", Path(__file__).parent / "target/debug/screen-oracle")).resolve()


def main():
    home = Path(tempfile.mkdtemp(prefix="hc-cwd-ui-", dir="/var/tmp"))
    project = home / "code/herdr"
    project.mkdir(parents=True)
    core = home / "core"
    env = H.isolated_env(home, core)
    env.update(HERDR_ENV="1", PS1="cwd$ ", HISTFILE=str(home / "history"),
               HERDR_CORE_CLIENT_ID="cwd-reattach")
    (home / "config.toml").write_text('[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n')
    config = core / "config/herdr-core"
    config.mkdir(parents=True)
    (config / "config.toml").write_text('shell=["/bin/bash","--noprofile","--norc"]\nnotes=false\n')
    previous_cwd = Path.cwd()
    viewer = rpc = None

    def hc(*args):
        return json.loads(subprocess.run([str(H.HC), "--json", *args], env=env,
                                        capture_output=True, text=True, check=True, timeout=15).stdout)

    def screen():
        return subprocess.run([str(ORACLE), "100", "28"], input=viewer.read(),
                              capture_output=True, check=True, timeout=5).stdout.decode()

    try:
        os.chdir(home)
        viewer = H.Viewer(env)
        H.until(lambda: "cwd$" in screen(), "initial shell")
        rpc = ORG.Rpc(env)
        before = rpc.doc()
        address = ORG.leaves(before["value"]["workspaces"][0]["tabs"][0]["root"])[0]
        initial = hc("get", address)
        pid, grid = initial["pid"], (initial["cols"], initial["rows"])
        assert initial["cwd"] == str(home)
        viewer.send(f"stty -echo; cd {shlex.quote(str(project))}; printf 'CWD_READY\\n'\r".encode())
        H.until(lambda: "CWD_READY" in screen(), "root shell changed directory")
        def project_record():
            record = hc("get", address)
            return record if (record.get("foreground") or {}).get("cwd") == str(project) else None

        current = H.until(project_record, "Core foreground cwd before reattach")
        assert current["cwd"] == str(home) and "last_cwd" not in current
        viewer.detach()
        viewer = H.Viewer(env)  # attach from HOME, not from the root shell's cwd
        H.until(lambda: "CWD_READY" in screen(), "reattached pane paint")
        viewer.send(b"\x02w")  # client-local chooser, no PTY input or Core focus request
        H.until(lambda: screen().splitlines()[0].strip().startswith("switch"), "first workspace chooser paint")
        text = screen()
        (home / "first-workspace-screen.txt").write_text(text)
        (home / "core-record.json").write_text(json.dumps(current, indent=2))
        assert re.search(r"· herdr\s*$", text, re.MULTILINE), text
        assert not any(line.strip() in {"menu", "settings", "keybinds", "detach"} for line in text.splitlines())
        assert rpc.doc() == before, "attach/chooser must not edit organization"
        final = hc("get", address)
        assert final["pid"] == pid and (final["cols"], final["rows"]) == grid
        assert final["cwd"] == str(home) and final["foreground"]["cwd"] == str(project)
        viewer.send(b"\x1b")
        H.until(lambda: not screen().splitlines()[0].strip().startswith("switch"), "chooser closed")
        viewer.send(b"\x02g")
        H.until(lambda: "Go to" in screen() and str(project) in screen(), "navigator uses current directory")
        text = screen()
        assert "search" not in text.lower() and "filter" not in text.lower()
        (home / "navigator-screen.txt").write_text(text)
        # Former editor/filter commands must not trap Esc or alter membership.
        viewer.send(b"/bwida\x7f")
        viewer.send(b"\x1b")
        H.until(lambda: "Go to" not in screen(), "single Esc closes navigator")
        assert rpc.doc() == before, "local navigator must not edit organization"
        print(json.dumps({"artifact_dir": str(home), "binary": str(H.HERDR),
                          "first_attach_workspace_cwd": True, "navigator_current_directory": True,
                          "chooser_no_menu": True, "navigator_no_search_or_filter": True,
                          "client_attach_cwd": str(home), "program_cwd": str(project),
                          "same_program_pid_and_grid": True, "no_document_write": True}))
    finally:
        if viewer:
            viewer.cleanup()
        if rpc:
            rpc.close()
        H.stop_private_core(env)
        os.chdir(previous_cwd)


if __name__ == "__main__":
    main()
