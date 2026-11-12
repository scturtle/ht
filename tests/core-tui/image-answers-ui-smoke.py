#!/usr/bin/env python3
"""Actual ht/Core/PTY: Emacs's image probe gets at most one timely answer.

No Emacs/model/SSH or user session is needed. Delays stop only an owned test ht;
this is a scheduler/late-reply control, not a WAN or physical-display measurement.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import tempfile
import time

import harness as H

PNG = b"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
ACK = b"\x1b_Gi=31;OK\x1b\\"
DA1 = b"\x1b[?62;22c"
PROBE = b"\x1b_Gi=31,a=q,t=d,f=100;" + PNG + b"\x1b\\\x1b[16t\x1b[c"
CASES = {"single": (1, []), "multi": (2, []), "late": (1, [0]), "mixed": (2, [0])}


def run_case(name):
    count, delayed = CASES[name]
    home = Path(tempfile.mkdtemp(prefix="hc-image-answers-", dir="/var/tmp"))
    result, go, stop = (home / file for file in ("replies.json", "go", "stop"))
    fixture = home / "probe.py"
    fixture.write_text(
        "#!/usr/bin/python3\n"
        "import json,os,select,time,tty\nfrom pathlib import Path\n"
        f"result,go,stop=map(Path,{[str(result), str(go), str(stop)]!r})\n"
        "tty.setraw(0)\n"
        "os.write(1,b'IMAGE_ANSWERS_READY')\n"
        "while not go.exists():\n"
        " if stop.exists():raise SystemExit\n"
        " time.sleep(.005)\n"
        "before=list(os.get_terminal_size(0))\n"
        f"start=time.monotonic();os.write(1,{PROBE!r})\n"
        "chunks=[]\n"
        "while time.monotonic()-start<1.0 and not stop.exists():\n"
        " if select.select([0],[],[],.03)[0]:\n"
        "  data=os.read(0,4096)\n"
        "  if not data:break\n"
        "  chunks.append({'ms':round((time.monotonic()-start)*1000,3),'hex':data.hex()})\n"
        "report={'chunks':chunks,'program_pid':os.getpid(),'grid_before':before,"
        "'grid_after':list(os.get_terminal_size(0))}\n"
        "pending=result.with_suffix('.pending');pending.write_text(json.dumps(report));pending.replace(result)\n"
        "os.write(1,b'IMAGE_ANSWERS_CAPTURED')\n"
    )
    fixture.chmod(0o700)
    env = H.isolated_env(home, home / "core")
    env["SHELL"] = str(fixture)
    (home / "config.toml").write_text(
        '[experimental]\nallow_nested=true\n[keys]\ndetach="f10"\n'
    )
    viewers, paused = [], set()
    try:
        for _ in range(count):
            viewer = H.Viewer(env)
            viewers.append(viewer)
            H.until(lambda: b"IMAGE_ANSWERS_READY" in viewer.read(), "owned fixture painted")
        for index in delayed:
            pid = viewers[index].pid
            os.kill(pid, signal.SIGSTOP)
            paused.add(pid)
            _, status = os.waitpid(pid, os.WUNTRACED)
            assert os.WIFSTOPPED(status), status
        go.touch()
        if delayed:
            time.sleep(.250)
            for pid in list(paused):
                os.kill(pid, signal.SIGCONT)
                paused.remove(pid)

        def captured():
            for view in viewers:
                view.read()
            return result.exists()

        H.until(captured, "probe's raw PTY replies captured")
        report = json.loads(result.read_text())
        joined = b"".join(bytes.fromhex(chunk["hex"]) for chunk in report["chunks"])
        expected = 0 if name == "late" else 1
        report.update(
            case=name, binary=str(H.HERDR), core_binary=str(H.HC), viewers=count,
            paused_viewer_indices=delayed, pause_ms=250 if delayed else 0,
            joined_repr=repr(joined), graphics_replies=joined.count(ACK),
            expected_graphics_replies=expected,
            no_resize_rescue=all(len(view.resizes) == 1 for view in viewers),
        )
        (home / "result.json").write_text(json.dumps(report, indent=2))
        print(json.dumps({"artifact_dir": str(home), **report}), flush=True)
        assert joined.count(DA1) == 1, report
        assert joined.count(ACK) == expected, report
        if expected:
            assert joined.index(ACK) < joined.index(DA1), report
        assert report["grid_before"] == report["grid_after"], report
        assert report["no_resize_rescue"], report
        for view in viewers:
            view.detach()
    finally:
        for pid in paused:
            try:
                os.kill(pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        stop.touch()
        try:
            for view in viewers:
                view.cleanup()
        finally:
            H.stop_private_core(env)
        assert not H.private_socket(env).exists()
    print(json.dumps({"case": name, "private_core_stopped": True}), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=["all", *CASES], default="all")
    args = parser.parse_args()
    for name in CASES if args.case == "all" else [args.case]:
        run_case(name)
