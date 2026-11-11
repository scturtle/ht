#!/usr/bin/env python3
"""Disposable raw Core-PTY protocol peer; never opens the host's terminal."""
import json
import os
from pathlib import Path
import select
import sys
import termios
import tty


def main():
    control, records = map(Path, sys.argv[1:])
    original = termios.tcgetattr(0)
    tty.setraw(0)
    phase = -1
    try:
        while True:
            try:
                requested = json.loads(control.read_text())["phase"]
            except (FileNotFoundError, json.JSONDecodeError):
                requested = phase
            if requested > phase:
                if requested not in range(4):
                    raise ValueError("invalid keyboard test phase")
                if phase < 0:
                    os.write(1, b"\x1b[=1;1u\x1b[>3u\x1b[>31u")
                    phase = 0
                while phase < requested:
                    os.write(1, b"\x1b[<u")
                    phase += 1
                os.write(1, f"\r\nKEY_PHASE_{phase}_READY\r\n".encode())
            if select.select([0], [], [], 0.02)[0]:
                data = os.read(0, 65536)
                if not data:
                    break
                with records.open("a") as log:
                    log.write(json.dumps({"phase": phase, "hex": data.hex()}) + "\n")
    finally:
        termios.tcsetattr(0, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
