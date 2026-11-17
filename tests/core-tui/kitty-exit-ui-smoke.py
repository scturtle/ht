#!/usr/bin/env python3
"""Owned virtual X11 + real Kitty + private loopback SSH + private Core."""

import asyncio
import ctypes as C
import shutil
import tempfile
import textwrap
import json
import os
from pathlib import Path
import select
import shlex
import socket
import subprocess as S
import sys
import time
import harness as H


async def latency_proxy(target, delay, ready):
    async def accept(a_reader, a_writer):
        b_reader, b_writer = await asyncio.open_connection("127.0.0.1", target)

        async def forward(reader, writer):
            queue = asyncio.Queue()

            async def receive():
                try:
                    while data := await reader.read(65536):
                        await queue.put((time.monotonic() + delay, data))
                finally:
                    await queue.put((0, b""))

            task = asyncio.create_task(receive())
            try:
                while True:
                    due, data = await queue.get()
                    if not data:
                        break
                    await asyncio.sleep(max(0, due - time.monotonic()))
                    writer.write(data)
                    await writer.drain()
            except (ConnectionError, asyncio.CancelledError):
                pass
            finally:
                task.cancel()
                writer.close()

        await asyncio.gather(forward(a_reader, b_writer), forward(b_reader, a_writer))

    server = await asyncio.start_server(accept, "127.0.0.1", 0)
    ready.write_text(str(server.sockets[0].getsockname()[1]))
    async with server:
        await server.serve_forever()


def main():
    BASE = Path(tempfile.mkdtemp(prefix="ht-kitty-", dir="/var/tmp"))
    print("Real Kitty/SSH artifacts:", BASE, flush=True)
    HOME = BASE / "h"
    HOME.mkdir(exist_ok=True)
    ENV = H.isolated_env(HOME, HOME / "c")
    (HOME / "config.toml").write_text('[keys]\ndetach="f10"\n')
    RUNS = int(os.environ.get("CORE_TUI_KITTY_EXIT_RUNS", "12"))
    DELAY = float(os.environ.get("CORE_TUI_SSH_DELAY_SECONDS", "0.3"))
    KITTY = os.environ.get("CORE_TUI_KITTY") or shutil.which("kitty")
    KITTEN = os.environ.get("CORE_TUI_KITTEN") or shutil.which("kitten")
    XVFB = os.environ.get("CORE_TUI_XVFB") or shutil.which("Xvfb")
    if not all((KITTY, KITTEN, XVFB)):
        raise SystemExit(
            "requires kitty, kitten and Xvfb (CORE_TUI_KITTY/KITTEN/XVFB override paths)"
        )
    processes = []
    files = []

    def proc(args, name, **kw):
        log = (BASE / (name + ".log")).open("wb")
        files.append(log)
        p = S.Popen(args, stdout=log, stderr=S.STDOUT, **kw)
        processes.append(p)
        return p

    wrapper = BASE / "outer.py"
    selectors = {
        k: v
        for k, v in ENV.items()
        if k
        in (
            "HOME",
            "PATH",
            "SHELL",
            "TERM",
            "CORE_TUI_HOME",
            "HERDR_CORE_BIN",
            "HC_SESSION",
            "XDG_RUNTIME_DIR",
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "HERDR_CONFIG_PATH",
        )
    }
    selectors["LANG"] = "C.UTF-8"
    wrapper.write_text(
        textwrap.dedent(
            """
    import json, os, select, subprocess, termios, time, tty
    from pathlib import Path
    base=Path("""
            + repr(str(BASE))
            + """)
    env="""
            + repr(selectors)
            + """
    for key in ('SSH_CONNECTION','SSH_CLIENT','SSH_TTY','TERM'):
        if key in os.environ:env[key]=os.environ[key]
    def record(path,value):
        tmp=path.with_suffix('.tmp')
        tmp.write_text(json.dumps(value))
        os.replace(tmp,path)
    for n in range("""
            + str(RUNS)
            + """):
        p=subprocess.Popen("""
            + repr(H.argv(ENV))
            + """,env=env)
        record(base/'ready.json',dict(iteration=n,pid=p.pid))
        result=p.wait()
        old=termios.tcgetattr(0)
        tty.setraw(0,termios.TCSANOW) # preserve all queued input, never TCSAFLUSH
        data=bytearray();end=time.monotonic()+1.5
        while time.monotonic()<end:
            if select.select([0],[],[],max(0,end-time.monotonic()))[0]:
                chunk=os.read(0,65536)
                if not chunk:break
                data.extend(chunk)
        termios.tcsetattr(0,termios.TCSANOW,old)
        (base/f'late-{n}.bin').write_bytes(data)
        record(base/f'done-{n}.json',dict(iteration=n,exit_code=result,late_bytes=len(data)))
        time.sleep(.15)
    """
        )
    )

    xlib = C.CDLL("libX11.so.6")
    xtst = C.CDLL("libXtst.so.6")
    xlib.XOpenDisplay.argtypes = [C.c_char_p]
    xlib.XOpenDisplay.restype = C.c_void_p
    xlib.XDefaultRootWindow.argtypes = [C.c_void_p]
    xlib.XDefaultRootWindow.restype = C.c_ulong
    xlib.XQueryTree.argtypes = [
        C.c_void_p,
        C.c_ulong,
        C.POINTER(C.c_ulong),
        C.POINTER(C.c_ulong),
        C.POINTER(C.POINTER(C.c_ulong)),
        C.POINTER(C.c_uint),
    ]
    xlib.XSetInputFocus.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_ulong]
    xlib.XFree.argtypes = [C.c_void_p]
    xlib.XFlush.argtypes = [C.c_void_p]
    xlib.XCloseDisplay.argtypes = [C.c_void_p]
    xtst.XTestFakeMotionEvent.argtypes = [
        C.c_void_p,
        C.c_int,
        C.c_int,
        C.c_int,
        C.c_ulong,
    ]
    xtst.XTestFakeButtonEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
    display = None
    try:
        r, w = os.pipe()
        proc(
            [
                str(XVFB),
                "-displayfd",
                str(w),
                "-screen",
                "0",
                "1000x700x24",
                "-nolisten",
                "tcp",
                "-noreset",
                "-ac",
            ],
            "xvfb",
            pass_fds=(w,),
        )
        os.close(w)
        assert select.select([r], [], [], 10)[0], "private Xvfb failed"
        display_number = os.read(r, 100).decode().strip()
        os.close(r)
        assert display_number.isdigit(), "private Xvfb did not start"
        display_name = ":" + display_number
        display = xlib.XOpenDisplay(display_name.encode())
        assert display
        for name in ("host", "user"):
            path = BASE / name
            if not path.exists():
                S.run(
                    [
                        "/usr/bin/ssh-keygen",
                        "-q",
                        "-t",
                        "ed25519",
                        "-N",
                        "",
                        "-f",
                        str(path),
                    ],
                    check=True,
                )
        (BASE / "authorized_keys").write_text((BASE / "user.pub").read_text())
        (BASE / "authorized_keys").chmod(0o600)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        (BASE / "sshd.conf").write_text(f"""Port {port}
    ListenAddress 127.0.0.1
    HostKey {BASE}/host
    PidFile {BASE}/sshd.pid
    AuthorizedKeysFile {BASE}/authorized_keys
    StrictModes no
    UsePAM no
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    PubkeyAuthentication yes
    AllowUsers {os.environ["USER"]}
    LogLevel VERBOSE
    """)
        sshd = proc(
            ["/usr/sbin/sshd", "-D", "-e", "-f", str(BASE / "sshd.conf")], "sshd"
        )

        def ssh_ready():
            if sshd.poll() is not None:
                raise RuntimeError("sshd failed: " + (BASE / "sshd.log").read_text())
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                    return True
            except OSError:
                return False

        H.until(ssh_ready, "private SSH listener")
        if DELAY:
            proc(
                [
                    sys.executable,
                    str(Path(__file__).resolve()),
                    "--latency-proxy",
                    str(port),
                    str(DELAY),
                    str(BASE / "proxy-port"),
                ],
                "proxy",
            )
            port = int(
                H.until(
                    lambda: (
                        (BASE / "proxy-port").exists()
                        and (BASE / "proxy-port").read_text().strip()
                    ),
                    "latency proxy",
                )
            )
        ssh = [
            "/usr/bin/ssh",
            "-F",
            "/dev/null",
            "-tt",
            "-p",
            str(port),
            "-i",
            str(BASE / "user"),
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "UserKnownHostsFile=" + str(BASE / "known_hosts"),
            "-o",
            "LogLevel=ERROR",
            os.environ["USER"] + "@127.0.0.1",
            "/usr/bin/python3 " + shlex.quote(str(wrapper)),
        ]
        gui_env = dict(ENV, DISPLAY=display_name, LIBGL_ALWAYS_SOFTWARE="1")
        gui_env.pop("WAYLAND_DISPLAY", None)
        kittysock = "unix:" + str(BASE / "kitty.sock")
        proc(
            [
                str(KITTY),
                "--config",
                "NONE",
                "--listen-on",
                kittysock,
                "-o",
                "allow_remote_control=yes",
                "-o",
                "linux_display_server=x11",
                "-o",
                "shell_integration=disabled",
                "-o",
                "sync_to_monitor=no",
                "-o",
                "initial_window_width=900",
                "-o",
                "initial_window_height=650",
                *ssh,
            ],
            "kitty",
            env=gui_env,
        )

        H.until(lambda: (BASE / "kitty.sock").exists(), "private Kitty socket")
        root = xlib.XDefaultRootWindow(display)
        rr = C.c_ulong()
        parent = C.c_ulong()
        children = C.POINTER(C.c_ulong)()
        count = C.c_uint()
        H.until(
            lambda: (
                xlib.XQueryTree(
                    display,
                    root,
                    C.byref(rr),
                    C.byref(parent),
                    C.byref(children),
                    C.byref(count),
                )
                and count.value
            ),
            "Kitty window",
        )
        xlib.XSetInputFocus(display, children[count.value - 1], 1, 0)
        xlib.XFlush(display)
        xlib.XFree(children)
        results = []
        for n in range(RUNS):
            H.until(
                lambda: (
                    (BASE / "ready.json").exists()
                    and json.loads((BASE / "ready.json").read_text())["iteration"] == n
                ),
                "ht launch " + str(n),
            )
            H.until(lambda: H.private_socket(ENV).exists(), "private Core")
            time.sleep(0.05 if n % 2 == 0 else 0.6)
            end = time.monotonic() + 0.2
            i = 0
            while time.monotonic() < end:
                xtst.XTestFakeMotionEvent(display, 0, 100 + i % 180, 150 + i % 160, 0)
                xlib.XFlush(display)
                i += 7
                time.sleep(0.005)
            S.run(
                [KITTEN, "@", "--to", kittysock, "send-key", "ctrl+d"],
                env=gui_env,
                check=True,
                timeout=10,
            )
            end = time.monotonic() + 6
            while time.monotonic() < end and not (BASE / f"done-{n}.json").exists():
                if n % 3 != 1:
                    xtst.XTestFakeMotionEvent(
                        display, 0, 100 + i % 180, 150 + i % 160, 0
                    )
                if n % 3 != 0:
                    for b in (4, 5):
                        xtst.XTestFakeButtonEvent(display, b, 1, 0)
                        xtst.XTestFakeButtonEvent(display, b, 0, 0)
                xlib.XFlush(display)
                i += 7
                time.sleep(0.004)
            H.until(
                lambda: (BASE / f"done-{n}.json").exists(),
                "post-exit capture " + str(n),
            )
            result = json.loads((BASE / f"done-{n}.json").read_text())
            result["gesture"] = ("motion", "wheel", "both")[n % 3]
            assert result["exit_code"] == 0, result
            assert result["late_bytes"] == 0, (
                result,
                repr((BASE / f"late-{n}.bin").read_bytes()[:200]),
            )
            results.append(result)
            print(json.dumps(result), flush=True)
        (BASE / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        log = (HOME / "c/herdr-client.log").read_text()
        assert (
            log.count("confirmed=true") == RUNS
            and log.count("pending_colors=0") == RUNS
        ), log
        print(
            json.dumps(
                dict(
                    real_kitty_loopback_ssh=True,
                    rtt_ms=round(DELAY * 2000),
                    cases=results,
                    artifacts=str(BASE),
                )
            )
        )
    finally:
        if display:
            xlib.XCloseDisplay(display)
        for p in reversed(processes):
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except S.TimeoutExpired:
                    p.kill()
                    p.wait()
        for f in files:
            f.close()
        H.stop_private_core(ENV)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--latency-proxy"]:
        asyncio.run(
            latency_proxy(int(sys.argv[2]), float(sys.argv[3]), Path(sys.argv[4]))
        )
    else:
        main()
