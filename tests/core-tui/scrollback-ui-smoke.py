#!/usr/bin/env python3
"""Configured client history across fresh attach and relay loss; private Core only."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unicodedata

import harness as H

SPEC = importlib.util.spec_from_file_location("scheduling", Path(__file__).with_name("scheduling-ui-smoke.py"))
SCHED = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCHED)
ORACLE = H.ROOT / "tests/core-tui/target/debug/screen-oracle"


def main():
    with tempfile.TemporaryDirectory(prefix="hc-scrollback-ui-", dir="/var/tmp") as scratch:
        home = Path(scratch)
        core = home / "core"
        env = H.isolated_env(home, core)
        fixture = home / "fixture.py"
        fixture.write_text('''import os,tty
tty.setraw(0)
print("SCROLLBACK_START",end="\\r\\n",flush=True)
while True:
    key=os.read(0,1)
    if key==b"b":
        for row in range(6000):
            print(f"{row:05} 中文 "+"x"*50,end="\\r\\n")
        print("SCROLLBACK_READY",end="\\r\\n",flush=True)
    elif key==b"x":
        print("INPUT_STILL_WORKS",end="\\r\\n",flush=True)
    elif not key:break
''')
        (core / "config/herdr-core").mkdir(parents=True)
        (core / "config/herdr-core/config.toml").write_text(
            "shell=" + json.dumps([sys.executable, "-u", str(fixture)]) + "\nnotes=false\nhistory_mb=64\n"
        )
        viewer = None
        dumps = set()
        results = {}

        def configure(lines):
            extra = "" if lines is None else f"[advanced]\nscrollback_limit_lines={lines}\n"
            (home / "config.toml").write_text(
                'onboarding=false\n[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n' + extra
            )

        def hc(*args):
            H.private_socket(env)
            result = subprocess.run([str(H.HC), "--json", *args], env=env, capture_output=True,
                                    text=True, check=True, timeout=15)
            return json.loads(result.stdout)

        def screen():
            return subprocess.run([str(ORACLE), "100", "28"], input=viewer.read(), capture_output=True,
                                  check=True, timeout=5).stdout.decode(errors="replace")

        def click(x, y, button=0):
            viewer.send(f"\x1b[<{button};{x+1};{y+1}M\x1b[<{button};{x+1};{y+1}m".encode())

        def capture():
            before = set(Path("/var/tmp").glob(f"ht-pane-dump-{viewer.pid}-*"))
            click(70, 20, 2)

            def item():
                for y, line in enumerate(screen().splitlines()):
                    if "Dump pane diagnostics" in line:
                        # The fixture has wide Chinese cells before the menu.
                        # Python character indices are not terminal columns.
                        prefix = line[:line.index("Dump pane diagnostics")]
                        column = sum(0 if unicodedata.combining(c) else
                                     2 if unicodedata.east_asian_width(c) in "WF" else 1
                                     for c in prefix)
                        return column + len("Dump pane diagnostics") // 2, y
                return None

            click(*H.until(item, "private pane diagnostic entry"))

            def completed():
                viewer.read()
                for path in set(Path("/var/tmp").glob(f"ht-pane-dump-{viewer.pid}-*")) - before:
                    dumps.add(path)
                    try:
                        report = json.loads((path / "report.json").read_text())
                        adapter = json.loads((path / "adapter.json").read_text())
                    except (OSError, ValueError):
                        continue
                    assert report["pane"] == address
                    assert report["errors"] == [], report
                    return adapter
                return None

            adapter = H.until(completed, "private history counter capture")
            assert adapter["scrollback_limit_lines"] == expected
            assert adapter["scrollback_limit_bytes"] is None
            metrics = adapter["view"]["terminal"]["scroll"]
            kept = metrics["total"] - metrics["rows"]
            if expected:
                assert expected <= kept <= expected + 1000, (expected, kept)
            else:
                assert kept == 0, kept
            return kept

        try:
            configure(None)
            expected = 1000
            viewer = H.Viewer(env)
            H.until(lambda: "SCROLLBACK_START" in screen(), "private fixture startup paint")
            address = hc("ls")["terminals"][0]["address"]
            original = hc("get", address)
            viewer.send(b"b")
            H.until(lambda: "SCROLLBACK_READY" in screen(), "history burst complete")
            results["default_live_rows"] = capture()
            viewer.detach()
            viewer = None

            # A fresh client must request more than the previous fixed 1000 rows.
            configure(5000)
            expected = 5000
            viewer = H.Viewer(env)
            H.until(lambda: "SCROLLBACK_READY" in screen(), "configured fresh attach")
            results["configured_fresh_attach_rows"] = capture()

            # Startup policy survives reconnect even if the config file changes.
            configure(7)
            # Kill only the relay verified as this owned viewer's child.
            socket = H.private_socket(env)
            relay = SCHED.owned_relay(viewer, socket)
            assert relay is not None
            pidfd = os.pidfd_open(relay)
            try:
                assert SCHED.owned_relay(viewer, socket) == relay
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            finally:
                os.close(pidfd)
            H.until(lambda: (new := SCHED.owned_relay(viewer, socket)) and new != relay,
                    "owned relay replacement")
            H.until(lambda: "SCROLLBACK_READY" in screen(), "configured recovered paint")
            results["configured_reconnect_rows"] = capture()
            viewer.detach()
            viewer = None

            configure(0)
            expected = 0
            viewer = H.Viewer(env)
            H.until(lambda: "SCROLLBACK_READY" in screen(), "zero-history attach")
            results["disabled_history_rows"] = capture()
            viewer.send(b"x")
            H.until(lambda: "INPUT_STILL_WORKS" in screen(), "input after history inspection")
            after = hc("get", address)
            assert all(after[k] == original[k] for k in ("pid", "cols", "rows"))
            viewer.detach()
            viewer = None
            results.update(same_program_pid_and_grid=True, input_after_capture=True,
                           private_core_only=True, scope="retained client physical rows, not Core capacity")
            print(json.dumps(results))
        except BaseException:
            evidence = Path(tempfile.mkdtemp(prefix="ht-scrollback-ui-failure-", dir="/var/tmp"))
            if viewer is not None:
                viewer.read()
                (evidence / "host-output.bin").write_bytes(viewer.output)
                (evidence / "screen.txt").write_text(screen())
            for log in home.rglob("herdr-client.log"):
                shutil.copy2(log, evidence / "herdr-client.log")
            print(f"private failure evidence: {evidence}", file=sys.stderr)
            raise
        finally:
            if viewer is not None:
                viewer.cleanup()
            H.stop_private_core(env)
            for path in dumps:
                shutil.rmtree(path)


if __name__ == "__main__":
    main()
