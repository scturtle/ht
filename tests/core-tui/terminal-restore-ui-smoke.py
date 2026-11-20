#!/usr/bin/env python3
"""Real TUI -> outer shell handoff on a private PTY that never answers color queries."""

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
COLOR_QUERY = re.compile(rb"\x1b\](10|11|4;[0-9]+);\?\x1b\\")
FULL_QUERY = [b"10", b"11"] + [f"4;{i}".encode() for i in range(256)]
PAINT_END = b"\x1b[?2026l"
EXIT_SIGNALS = {
    "sigint": signal.SIGINT,
    "sigterm": signal.SIGTERM,
    "sighup": signal.SIGHUP,
}


def wait_for(viewer, check, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check(viewer.read()):
            return
    raise AssertionError(
        f"timeout: {description}; tail={bytes(viewer.output[-500:])!r}"
    )


def reporting_modes_at_exit(output):
    modes = {}
    prefix = output[: output.index(b"HT_EXIT_STATUS=")]
    for match in re.finditer(rb"\x1b\[\?([0-9;]+)([hl])", prefix):
        for value in match[1].split(b";"):
            modes[int(value)] = match[2] == b"h"
    return modes


def run_case(kind, base):
    home = base / "h"
    home.mkdir()
    (home / "config.toml").write_text('[keys]\ndetach="f10"\nreload_config="f6"\n')
    env = H.isolated_env(home, home / "c")
    pid_file = base / "ht.pid"
    env.update(RESTORE_HT=str(H.HERDR), RESTORE_PID_FILE=str(pid_file))
    wrapper = base / "outer.py"
    wrapper.write_text(f"""#!{sys.executable}
import os
from pathlib import Path
import subprocess
import sys
import termios
before = termios.tcgetattr(0)
child = subprocess.Popen([os.environ["RESTORE_HT"], *sys.argv[1:]])
Path(os.environ["RESTORE_PID_FILE"]).write_text(str(child.pid))
status = child.wait()
print(f"\\nHT_EXIT_STATUS={{status}} TTY_RESTORED={{termios.tcgetattr(0) == before}}", flush=True)
env = dict(os.environ, PS1="HT_EXIT_SHELL$ ")
os.execve("/bin/bash", ["bash", "--noprofile", "--norc", "-i"], env)
""")
    wrapper.chmod(0o700)
    viewer = None
    try:
        viewer = H.Viewer(env, binary=wrapper)
        wait_for(
            viewer,
            lambda out: b"\x1b]4;255;?\x1b\\" in out and PAINT_END in out,
            "palette query and first paint",
        )
        assert COLOR_QUERY.findall(viewer.read()) == FULL_QUERY
        batches = 1
        if kind == "detach":
            # Resize, focus, and an unsolicited appearance report are not queries.
            start = len(viewer.output)
            viewer.resize(92, 25)
            wait_for(viewer, lambda out: PAINT_END in out[start:], "paint after host resize")
            viewer.send(b"\x1b[O\x1b[I\x1b[?997;2n")
            viewer.send(b"\x1b[17~")  # F6, explicit config reload.
            wait_for(viewer, lambda out: COLOR_QUERY.findall(out) == FULL_QUERY * 2,
                     "one more full color query batch after reload")
            batches = 2

        started = time.monotonic()
        if kind in EXIT_SIGNALS:
            os.kill(int(pid_file.read_text()), EXIT_SIGNALS[kind])
        else:
            viewer.send(b"\x1b[21~" if kind == "detach" else b"\x04")
        # Send no color or other terminal replies: handoff must not wait for them.
        wait_for(viewer, lambda out: PROMPT in out,
                 f"{kind}: outer shell handoff without terminal replies", timeout=1.8)
        elapsed = time.monotonic() - started
        output = viewer.read()
        assert b"HT_EXIT_STATUS=0 TTY_RESTORED=True" in output, output[-500:]
        modes = reporting_modes_at_exit(output)
        assert not any(modes.get(mode, False) for mode in (9, 1000, 1002, 1003, 1004, 1049, 2004)), modes
        assert b"\x1bP+q" not in output, "client must not send a handoff query"
        assert not re.search(rb"\x1b\[\?(996n|2031[hl])", output)
        assert COLOR_QUERY.findall(output) == FULL_QUERY * batches, (
            "unexpected color query outside startup and reload"
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
        viewer.send(b"exit 0\n")
        viewer.wait_exit()
    finally:
        if viewer is not None:
            if viewer.pid and PROMPT in viewer.output:
                viewer.send(b"\x03exit 0\n")
            viewer.cleanup()
        H.stop_private_core(env)
    return dict(
        case=kind,
        color_query_batches=batches,
        tty_restored=True,
        shell_input_preserved=True,
        shutdown_seconds=round(elapsed, 3),
        private_core_stopped=not H.private_socket(env).exists(),
    )


def main():
    results = []
    for kind in (*EXIT_SIGNALS, "detach", "last-pane"):
        with tempfile.TemporaryDirectory(prefix="ht-exit-", dir="/var/tmp") as scratch:
            results.append(run_case(kind, Path(scratch)))
    print(json.dumps(dict(terminal_handoff_without_replies=True, cases=results)))


if __name__ == "__main__":
    main()
