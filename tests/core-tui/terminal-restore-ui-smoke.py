#!/usr/bin/env python3
"""Real TUI -> outer shell handoff with delayed, split host palette replies.

The delay/fragmentation is synthetic, not an actual SSH/WAN measurement. Every
case owns a private Core and PTY; no inherited session is touched.
"""

import json
import os
from pathlib import Path
import re
import signal
import sys
import tempfile
import time

import harness as H


PROMPT = b"HT_EXIT_SHELL$ "
PROOF = b"SHELL_INPUT_PRESERVED"


def wait_for(viewer, check, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check(viewer.read()):
            return
    raise AssertionError(
        f"timeout: {description}; tail={bytes(viewer.output[-500:])!r}"
    )


def observe_shell(viewer, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        viewer.read()
    return PROMPT in viewer.output


def shutdown_fence(output):
    return re.search(rb"\x1bP\+q([0-9a-f]+)\x1b\\", output)


def reporting_modes_at_fence(output):
    modes = {}
    prefix = output[: shutdown_fence(output).start()]
    for match in re.finditer(rb"\x1b\[\?([0-9;]+)([hl])", prefix):
        for value in match[1].split(b";"):
            modes[int(value)] = match[2] == b"h"
    return modes


def run_case(kind, base):
    home = base / "h"
    home.mkdir()
    (home / "config.toml").write_text('[keys]\ndetach="f10"\n')
    env = H.isolated_env(home, home / "c")
    pid_file = base / "ht.pid"
    env.update(RESTORE_HT=str(H.HERDR), RESTORE_PID_FILE=str(pid_file))
    wrapper = base / "outer.py"
    wrapper.write_text(f"""#!{sys.executable}
import os
from pathlib import Path
import subprocess
import sys
child = subprocess.Popen([os.environ["RESTORE_HT"], *sys.argv[1:]])
Path(os.environ["RESTORE_PID_FILE"]).write_text(str(child.pid))
status = child.wait()
print(f"\\nHT_EXIT_STATUS={{status}}", flush=True)
env = dict(os.environ, PS1="HT_EXIT_SHELL$ ")
os.execve("/bin/bash", ["bash", "--noprofile", "--norc", "-i"], env)
""")
    wrapper.chmod(0o700)
    viewer = None
    result = {}
    try:
        viewer = H.Viewer(env, binary=wrapper)
        wait_for(
            viewer,
            lambda out: b"\x1b]4;255;?\x1b\\" in out and b"\x1b[?2026l" in out,
            "palette query and first paint",
        )
        if kind in ("last-pane-aged", "last-pane-slow"):
            # The live parser's one-second ambiguity window is not evidence
            # that an unanswered query completed across SSH.
            observe_shell(viewer, 1.2)
        # The existing slow case also exercises an aged OSC 11 prefix.
        first = (
            b"\x1b]11;rgb:0000/0000/0000\x1b\\"
            if kind == "last-pane-slow"
            else b"\x1b]4;0;rgb:0000/afaf/d7d7\x1b\\"
        )
        prefix_len = 8
        if kind != "unanswered":
            # Default colors and cell-size replies do not remain outstanding.
            data = b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07"
            data += b"\x1b[6;16;8t"
            if kind == "detach":
                # Dispatch detach before buffering the partial OSC reply.
                data += b"\x1b[21~"
            elif kind.startswith("last-pane"):
                # Close the actual shell in the only pane, not the ht process.
                if kind in ("last-pane-aged", "last-pane-slow"):
                    data = b""
                data += b"\x04"
            viewer.send(
                data + (b"" if kind == "last-pane-early-ack" else first[:prefix_len])
            )
        started = time.monotonic()
        if kind in ("signal", "unanswered"):
            os.kill(int(pid_file.read_text()), signal.SIGTERM)

        early_shell = False
        delayed_bytes = 0
        wait_for(viewer, shutdown_fence, f"{kind}: uniquely identified shutdown query")
        fence = shutdown_fence(viewer.read())
        fence_reply = b"\x1bP0+r" + fence[1] + b"\x1b\\"
        modes = reporting_modes_at_fence(viewer.read())
        capture_at_fence = any(modes.get(mode, False) for mode in (9, 1000, 1002, 1003))
        alt_at_fence = modes.get(1049, False)
        assert not alt_at_fence, "alternate-screen wheel-to-arrow producer still active"
        assert not capture_at_fence, "mouse capture must stop before the shutdown fence"
        wheel = b"\x1b[<64;66;20M\x1b[<65;66;20M\x1b[<65;66;20M"
        if kind == "last-pane-early-ack":
            # A status response is not a per-query color acknowledgement. Keep
            # the palette tail in flight after it while motion packets arrive.
            viewer.send(b"\x1b[0n\x1b[3n\x1b[?997;1n" + fence_reply)
        if kind != "unanswered":
            # Delay the response to the fresh fence independently of the live
            # parser's expired query window, including an SSH-like >1s stall.
            early_shell |= observe_shell(
                viewer, 1.1 if kind == "last-pane-slow" else 0.15
            )
            tail = (
                first if kind == "last-pane-early-ack" else first[prefix_len:]
            ) + b"".join(
                f"\x1b]4;{index};rgb:0000/afaf/d7d7\x1b\\".encode()
                for index in range(0 if kind == "last-pane-slow" else 1, 256)
            )
            if kind in ("last-pane-aged", "last-pane-slow"):
                tail += b"\x1b]10;rgb:ffff/ffff/ffff\x07"
                if kind != "last-pane-slow":
                    tail += b"\x1b]11;rgb:0000/0000/0000\x07"
                tail += b"\x1b[6;16;8t"
            if kind != "last-pane-early-ack":
                tail += (
                    b"\x1b[?997;1n"  # Outstanding appearance query, not a new query.
                )
            delayed_bytes = len(tail)
            # Split the first reply's ST between ESC and backslash, then cross a
            # 4096-byte read boundary with the rest of the 256-entry palette.
            limits = [len(first) - prefix_len - 1, 1, 4096, len(tail)]
            for limit in limits:
                chunk, tail = tail[:limit], tail[limit:]
                if chunk:
                    viewer.send(chunk)
                if tail:
                    early_shell |= observe_shell(viewer, 0.05)
            assert not tail
            # Even complete color counts cannot release the tty before the
            # terminal confirms the ordering fence. Split that reply too.
            if kind != "last-pane-early-ack":
                early_shell |= observe_shell(viewer, 0.1)
                if kind == "last-pane-wheel":
                    # Reports generated before capture-disable can still be in flight.
                    for chunk in [wheel[:1], wheel[1:3], wheel[3:11], wheel[11:]]:
                        viewer.send(chunk)
                        viewer.read()
                viewer.send(fence_reply[:2])
                early_shell |= observe_shell(viewer, 0.05)
                viewer.send(fence_reply[2:])

        wait_for(
            viewer, lambda out: PROMPT in out, f"{kind}: outer shell handoff", timeout=4
        )
        elapsed = time.monotonic() - started
        output = viewer.read()
        shell_output = output[output.index(PROMPT) :]
        assert b"HT_EXIT_STATUS=0" in output, output[-500:]
        client_log = (home / "c" / "herdr-client.log").read_text()
        assert "terminal.shutdown_fence" in client_log, client_log
        assert f"confirmed={str(kind != 'unanswered').lower()}" in client_log, (
            client_log
        )
        if kind != "unanswered":
            assert "pending_colors=0" in client_log, client_log
        assert not early_shell, "shell resumed before requested replies arrived"
        assert b"rgb:" not in shell_output, shell_output[-500:]
        assert b";66;20M" not in shell_output, shell_output[-500:]
        assert elapsed < 3.5, f"shutdown was not bounded: {elapsed:.3f}s"
        if kind != "unanswered":
            assert output.count(b"\x1b]4;255;?\x1b\\") == 1, (
                "shutdown issued another palette query"
            )

        # No detached stdin thread may consume the first command typed in the
        # outer shell. Hex escapes ensure the marker cannot match command echo.
        command = (
            b"printf '" + b"".join(f"\\x{ch:02x}".encode() for ch in PROOF) + b"\\n'\n"
        )
        viewer.send(command)
        wait_for(
            viewer,
            lambda out: PROOF + b"\r\n" in out,
            "first shell command survives input ownership handoff",
        )
        result = dict(
            case=kind,
            delayed_reply_bytes=delayed_bytes,
            shell_rgb_leak=False,
            shell_mouse_leak=False,
            shell_input_preserved=True,
            shutdown_fence_answered=kind != "unanswered",
            mouse_capture_at_fence=capture_at_fence,
            alternate_screen_at_fence=alt_at_fence,
            shutdown_seconds=round(elapsed, 3),
        )
        viewer.send(b"exit 0\n")
        viewer.wait_exit()
    finally:
        if viewer is not None:
            if viewer.pid and PROMPT in viewer.output:
                # Only this gate-owned shell: abort synthetic input, never run it.
                viewer.send(b"\x03exit 0\n")
            viewer.cleanup()
        H.stop_private_core(env)
    result["private_core_stopped"] = not H.private_socket(env).exists()
    return result


def main():
    results = []
    for kind in (
        "signal",
        "detach",
        "last-pane",
        "last-pane-aged",
        "last-pane-slow",
        "last-pane-wheel",
        "last-pane-early-ack",
        "unanswered",
    ):
        with tempfile.TemporaryDirectory(prefix="ht-exit-", dir="/var/tmp") as scratch:
            results.append(run_case(kind, Path(scratch)))
    print(json.dumps(dict(delayed_host_reply_handoff=True, cases=results)))


if __name__ == "__main__":
    main()
