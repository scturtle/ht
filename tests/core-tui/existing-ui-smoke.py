#!/usr/bin/env python3
"""Compatibility import facade; the obsolete initial-milestone assertions are retired.

Run directly to execute the current organization gate. Shared process/selector
machinery lives in harness.py, including the exec-before-first-paint geometry.
"""
from pathlib import Path
from harness import (ROOT, HC, HERDR, Viewer, argv, core_session, isolated_env,
                     private_socket, stop_private_core, until)


if __name__ == "__main__":
    import runpy
    runpy.run_path(str(Path(__file__).with_name("organization-ui-smoke.py")),run_name="__main__")
