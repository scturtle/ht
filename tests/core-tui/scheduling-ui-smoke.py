#!/usr/bin/env python3
"""Owned real-PTY idle/wake/latency gate. Optional CORE_UI_BASELINE enables A/B.

This measures local input-to-host-output latency, NOT WAN RTT or model activity.
It starts and stops only uniquely named Core sessions under its own temp directory.
"""
import importlib.util
import json
import math
import os
from pathlib import Path
import statistics
import signal
import subprocess
import sys
import tempfile
import time

SPEC = importlib.util.spec_from_file_location("ui", Path(__file__).with_name("existing-ui-smoke.py"))
UI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(UI)


def counters(pid):
    for task in Path(f"/proc/{pid}/task").iterdir():
        if (task / "comm").read_text().strip() == "core-endpoint":
            status = dict(line.split(":", 1) for line in (task / "status").read_text().splitlines()
                          if ":" in line)
            return int(status["voluntary_ctxt_switches"])
    raise AssertionError("owned client's Core worker was not found")


def pump(viewer, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        viewer.read()


def owned_relay(viewer, socket):
    children=set()
    for task in Path(f"/proc/{viewer.pid}/task").iterdir():
        try:
            children.update((task / "children").read_text().split())
        except FileNotFoundError:
            continue  # The old reader/writer threads can finish during recovery.
    for pid in children:
        try:
            process=Path(f"/proc/{pid}")
            args=(process / "cmdline").read_bytes().split(b"\0")
            if len(args)>1 and args[1]==b"connect" and Path(os.fsdecode(args[0])).resolve()==UI.HC.resolve():
                assert f"HC_SOCKET={socket}".encode() in (process / "environ").read_bytes().split(b"\0")
                return int(pid)
        except FileNotFoundError:
            continue
    return None


def check_relay_recovery(viewer,hc,socket,pid):
    # Build a nontrivial projection revision before dropping ONLY our relay.
    # Core and the fixture stay alive; no split, resize, focus or attach is used
    # as a manual recovery action.
    address=hc("ls")["terminals"][0]["address"]
    for index in range(8):
        start_at=len(viewer.output)
        hc("label",address,"herdr.display_name="+chr(65+index)*12)
        UI.until(lambda:b"\x1b[?2026l" in viewer.read()[start_at:],"label metadata projection")
    pump(viewer,.2)
    relay=owned_relay(viewer,socket)
    assert relay is not None
    start_at=len(viewer.output)
    pidfd=os.pidfd_open(relay)
    try:
        assert owned_relay(viewer,socket)==relay
        signal.pidfd_send_signal(pidfd,signal.SIGKILL)
    finally:
        os.close(pidfd)
    UI.until(lambda:(new:=owned_relay(viewer,socket)) and new!=relay,"replacement existing-only relay")
    UI.until(lambda:b"\x1b[?2026l" in viewer.read()[start_at:],"post-recovery projection")
    start_at=len(viewer.output)
    viewer.send(b"x")
    deadline=time.monotonic()+3
    while b"U"*8 not in viewer.output[start_at:]:
        if time.monotonic()>=deadline:
            raise AssertionError(f"relay recovery lost its projection: core={hc('read',address,'--lines','0')}; "
                                 f"host_tail={bytes(viewer.output[start_at:])[-800:]!r}")
        viewer.read()
    assert hc("get",address)["pid"]==pid


def run(binary, strict):
    with tempfile.TemporaryDirectory(prefix="hc-scheduling-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        core = home / "core"
        session = UI.core_session(str(core))
        socket = core / "run/herdr-core" / f"{session}.sock"
        script = home / "fixture.py"
        script.write_text('''import os, sys, tty
tty.setraw(0)
sys.stdout.write("\\x1b[2J\\x1b[HFIXTURE_READY")
sys.stdout.flush()
n = 0
while True:
    data = os.read(0, 1)
    if not data:
        break
    if data == b"x":
        sys.stdout.write("\\x1b[H" + chr(65 + n % 26) * 12)
        sys.stdout.flush()
        n += 1
''')
        env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_", "PI_"))
               and k not in {"DISPLAY", "WAYLAND_DISPLAY", "BASH_ENV", "ENV", "ZDOTDIR", "TMUX", "STY",
                             "SSH_CONNECTION", "SSH_TTY", "VSCODE_IPC_HOOK_CLI"}}
        env.update(HOME=str(home), SHELL="/bin/bash", TERM="xterm-256color",
                   CORE_TUI_HOME=str(core), HERDR_CORE_BIN=str(UI.HC), HC_SESSION=session,
                   XDG_RUNTIME_DIR=str(core / "run"), XDG_STATE_HOME=str(core / "state"),
                   XDG_CONFIG_HOME=str(core / "config"), HERDR_CONFIG_PATH=str(home / "config.toml"))
        if "HERDR_ENV" in os.environ:
            env["HERDR_ENV"] = os.environ["HERDR_ENV"]
        (home / "config.toml").write_text('onboarding=false\n[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n')
        (core / "config/herdr-core").mkdir(parents=True)
        (core / "config/herdr-core/config.toml").write_text(
            "shell=" + json.dumps([sys.executable, "-u", str(script)]) + "\nnotes=false\n")
        viewer = None

        def hc(*args):
            result = subprocess.run([str(UI.HC), "--json", *args], env=env,
                                    capture_output=True, text=True, check=True, timeout=15)
            return json.loads(result.stdout)

        try:
            viewer = UI.Viewer(env,binary)
            UI.until(lambda: b"FIXTURE_READY" in viewer.read(), "fixture first paint")
            address = hc("ls")["terminals"][0]["address"]
            pid = hc("get", address)["pid"]
            pump(viewer, 1)
            before = counters(viewer.pid)
            output_start = len(viewer.output)
            start = time.monotonic()
            pump(viewer, 3)
            elapsed = time.monotonic() - start
            wakes = counters(viewer.pid) - before
            idle_bytes = len(viewer.output) - output_start
            latencies = []
            for index in range(20):
                start_at = len(viewer.output)
                start = time.monotonic_ns()
                viewer.send(b"x")
                # A few initial cells may already match the first token; require
                # a changed run plus its synchronized-frame end, not a full row.
                marker = bytes([65 + index]) * 8
                deadline = time.monotonic() + 3
                while (marker not in viewer.output[start_at:] or
                       b"\x1b[?2026l" not in viewer.output[start_at:][viewer.output[start_at:].find(marker):]):
                    if time.monotonic() >= deadline:
                        raise AssertionError(f"input {index} failed to reach host without resize/focus; "
                                             f"core={hc('read', address, '--lines', '0')}; "
                                             f"host_tail={bytes(viewer.output[start_at:])[-1600:]!r}")
                    viewer.read()
                latencies.append((time.monotonic_ns() - start) / 1e6)
            assert hc("get", address)["pid"] == pid, "fixture must not restart"
            if strict:
                assert wakes <= 5, ("idle worker is polling", wakes, elapsed)
                assert idle_bytes == 0, ("idle client painted unchanged frames", idle_bytes)
                assert max(latencies) < 1000, latencies
                check_relay_recovery(viewer,hc,socket,pid)
            viewer.detach()
            return {"binary": str(binary), "idle_seconds": round(elapsed, 3),
                    "worker_voluntary_switches": wakes, "worker_switches_per_second": round(wakes / elapsed, 2),
                    "idle_host_output_bytes": idle_bytes,
                    "input_samples": len(latencies), "input_to_host_median_ms": round(statistics.median(latencies), 3),
                    "input_to_host_p95_ms": round(sorted(latencies)[math.ceil(.95 * len(latencies)) - 1], 3),
                    "same_program_pid": True, "no_geometry_or_focus_recovery": True,
                    "relay_recovery_after_metadata_changes": True if strict else "not checked"}
        finally:
            if viewer is not None:
                viewer.cleanup()
            if socket.exists():
                assert str(socket).startswith(str(home) + "/")
                subprocess.run([str(UI.HC), "server", "stop"], env=env,
                               capture_output=True, check=True, timeout=15)


def main():
    report = {}
    if baseline := os.environ.get("CORE_UI_BASELINE"):
        report["before"] = run(Path(baseline).resolve(strict=True), False)
    report["after"] = run(UI.HERDR, True)
    report["scope"] = "local private PTY; worker switches and bytes are measured only while idle; latency is not WAN RTT"
    print(json.dumps(report))


if __name__ == "__main__":
    main()
