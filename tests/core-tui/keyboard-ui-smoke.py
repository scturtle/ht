#!/usr/bin/env python3
"""Real default TUI: Kitty press/repeat/release across replacement and stack pops."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    sys.modules[name] = result
    spec.loader.exec_module(result)
    return result


UI = load("keyboard_existing_ui", HERE / "existing-ui-smoke.py")
ORG = load("keyboard_organization_ui", HERE / "organization-ui-smoke.py")


def screen(viewer):
    output = viewer.read() if viewer.pid else bytes(viewer.output)
    return subprocess.run([str(ORG.ORACLE), "100", "28", json.dumps(viewer.resizes)], input=output,
                          capture_output=True, timeout=5, check=True).stdout.decode(errors="replace")


def facts(env, address):
    result = subprocess.run([str(UI.HC), "--json", "get", address], env=env,
                            capture_output=True, text=True, timeout=15, check=True)
    return json.loads(result.stdout)


def main():
    private = Path(tempfile.mkdtemp(prefix="hc-keyboard-ui-", dir="/var/tmp"))
    viewers, rpc = [], None
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_"))}
    for name in ("DISPLAY", "WAYLAND_DISPLAY", "SSH_TTY", "SSH_CONNECTION", "TMUX", "TMUX_PANE"):
        env.pop(name, None)
    core = private / "core"
    for path in [core / "run", core / "config", core / "state"]:
        path.mkdir(parents=True, mode=0o700)
    config = private / "ui.toml"
    config.write_text('onboarding = false\n[experimental]\nallow_nested = true\n[ui]\nsidebar_start_collapsed = true\nsidebar_collapsed_mode = "hidden"\n[keys]\ndetach = "f10"\n')
    (core / "config/herdr-core").mkdir(mode=0o700)
    (core / "config/herdr-core/config.toml").write_text('notes = false\n')
    env.update(HOME=str(private), XDG_RUNTIME_DIR=str(core / "run"),
               XDG_STATE_HOME=str(core / "state"), XDG_CONFIG_HOME=str(core / "config"),
               HERDR_CORE_BIN=UI.HC, CORE_TUI_HOME=str(core), HERDR_CONFIG_PATH=str(config),
               HERDR_CORE_CLIENT_ID="keyboard-smoke", HC_SESSION="gate-" + __import__("hashlib").sha1(str(core).encode()).hexdigest()[:12], TERM="xterm-256color")
    control, records = private / "control.json", private / "records.jsonl"
    control.write_text('{"phase":0}')
    try:
        rpc = ORG.Rpc(env)
        reply = rpc.request("terminal.run", {"argv": [sys.executable, str(HERE / "keyboard-program.py"), str(control), str(records)],
                                               "cwd": str(private), "labels": {"herdr": "tui"},
                                               "no_parent": True, "cols": 100, "rows": 27,
                                               "env": {"TERM": "xterm-ghostty"}})
        assert "error" not in reply, reply
        record = reply["result"]
        address, pid = record["address"], record["pid"]
        current = UI.Viewer(env)
        viewers.append(current)
        UI.until(lambda: "KEY_PHASE_0_READY" in screen(current), "initial full Kitty mode replay")

        def phase_bytes(phase):
            if not records.exists():
                return b""
            rows = records.read_text().splitlines()
            result = b""
            for row in rows:
                try:
                    value = json.loads(row)
                except json.JSONDecodeError:
                    continue
                if value["phase"] == phase:
                    result += bytes.fromhex(value["hex"])
            return result

        def probe(phase):
            sentinel = f"END_KEY_PHASE_{phase}".encode()
            current.send(b"\x1b[97;5:1u\x1b[97;5:2u\x1b[97;5:3u")
            # Paste is an ordered literal input barrier, independent of key mode.
            current.send(b"\x1b[200~" + sentinel + b"\x1b[201~")
            UI.until(lambda: sentinel in phase_bytes(phase), f"phase {phase} input barrier")
            data = phase_bytes(phase).split(sentinel, 1)[0]
            if phase < 2:
                assert len(re.findall(rb"\x1b\[97;5", data)) == 3, (phase, data)
                assert b":2" in data and b":3" in data, (phase, data)
            elif phase == 2:
                assert len(re.findall(rb"\x1b\[97;5", data)) == 2 and b":3" not in data, data
            else:
                assert data == b"\x01\x01", data
            return data.hex()

        evidence = [probe(0)]
        current.detach()
        assert facts(env, address)["pid"] == pid
        current = UI.Viewer(env)
        viewers.append(current)
        UI.until(lambda: "KEY_PHASE_0_READY" in screen(current), "replacement client full Kitty stack replay")
        for phase in range(1, 4):
            pending = control.with_suffix(".pending")
            pending.write_text(json.dumps({"phase": phase}))
            pending.replace(control)
            UI.until(lambda: f"KEY_PHASE_{phase}_READY" in screen(current), f"stack pop to phase {phase}")
            evidence.append(probe(phase))
        assert facts(env, address)["pid"] == pid
        current.detach()
        result = {"default_tui": True, "same_program_pid_after_client_replacement": True,
                  "kitty_press_repeat_release": True, "nested_stack_pops_after_replacement": True,
                  "legacy_fallback_after_final_pop": True, "phase_input_hex": evidence,
                  "server_restart_handoff_tested": False, "full_migration_acceptance": False}
    except Exception:
        for viewer in viewers:
            print(screen(viewer)[-7000:], file=sys.stderr)
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
                # Fail loudly and retain the directory if shutdown is not confirmed.
                subprocess.run([str(UI.HC), "server", "stop"], env=env,
                               capture_output=True, text=True, timeout=15, check=True)
                UI.until(lambda: not (core / "core.sock").exists(), "private Core socket removed after shutdown")
                shutil.rmtree(private)
    print(json.dumps({**result, "private_core_stopped": True}))


if __name__ == "__main__":
    main()
