#!/usr/bin/env python3
"""Launcher regression tests; private fake programs, no Core server or host tty."""
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

LAUNCHER_DIR = Path(__file__).resolve().parents[2] / "scripts/core-tui"


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="core-launcher-", dir="/var/tmp")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        scripts = self.root / "scripts/core-tui"
        scripts.mkdir(parents=True)
        self.launcher = scripts / "run.sh"
        shutil.copy2(LAUNCHER_DIR / "run.sh", self.launcher)
        shutil.copy2(LAUNCHER_DIR / "run.py", scripts / "run.py")
        tools = self.root / ".local/toolchain"
        tools.mkdir(parents=True)
        self.executable(tools / "just", "#!/bin/sh\nexit 0\n")
        self.executable(self.root / "target/release/ht", '''#!/usr/bin/env python3
import json, os, sys
print(json.dumps({"stdin_tty": os.isatty(0), "stdout_tty": os.isatty(1),
                  "argv": sys.argv[1:],
                  "inherited_home": os.environ.get("HERDR_CORE_HOME"),
                  "inherited_socket": os.environ.get("HERDR_SOCKET_PATH"),
                  "client_id": os.environ.get("HERDR_CORE_CLIENT_ID"),
                  "input": sys.stdin.readline().rstrip("\\n")}), flush=True)
''')
        self.core = self.root / "fake-core"
        self.executable(self.core, '''#!/usr/bin/env python3
import json, os, sys
print(json.dumps({"args": sys.argv[1:], "socket": os.environ.get("HC_SOCKET")}), flush=True)
''')
        self.home = self.root / "private-core"
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "HERDR_", "CORE_UI_", "CORE_TUI_"))}
        self.env.update(CORE_TUI_HC=str(self.core), CORE_UI_HOME=str(self.home),
                        HERDR_SOCKET_PATH="must-not-be-inherited", HC_SOCKET="must-not-be-used")

    def executable(self, path, source):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        path.chmod(0o700)

    def test_piped_input_is_not_replaced_by_python_program_text(self):
        result = subprocess.run(["sh", str(self.launcher)], input=b"PIPE_INPUT_PROOF\n",
                                env=self.env, capture_output=True, timeout=10, check=True)
        record = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(record["input"], "PIPE_INPUT_PROOF")
        self.assertEqual(record["argv"], ["--core-home", str(self.home)])
        # The home must be an argument, not something the environment can move.
        self.assertIsNone(record["inherited_home"])
        self.assertIsNone(record["inherited_socket"])

    def test_only_explicit_launcher_identity_selects_persistent_client_preferences(self):
        for explicit in [None, "zoom-client"]:
            env = dict(self.env, HERDR_CORE_CLIENT_ID="must-not-inherit-outer-client")
            if explicit is not None:
                env["CORE_UI_CLIENT_ID"] = explicit
            result = subprocess.run(["sh", str(self.launcher)], input=b"IDENTITY\\n",
                                    env=env, capture_output=True, timeout=10, check=True)
            record = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(record["client_id"], explicit)

    def test_terminal_stdin_and_actual_typing_reach_the_program(self):
        pid, fd = pty.fork()
        if pid == 0:
            os.execve("/bin/sh", ["sh", str(self.launcher)], self.env)
        waited = False
        try:
            os.write(fd, b"KEYBOARD_INPUT_PROOF\n")
            output = bytearray()
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if select.select([fd], [], [], 0.05)[0]:
                    try:
                        data = os.read(fd, 65536)
                    except OSError:
                        break
                    if not data:
                        break
                    output.extend(data)
            else:
                self.fail("launcher did not finish")
            _, status = os.waitpid(pid, 0)
            waited = True
            self.assertEqual(os.waitstatus_to_exitcode(status), 0, output)
            records = [json.loads(line) for line in output.splitlines() if line.startswith(b'{"stdin_tty"')]
            self.assertEqual(len(records), 1, output)
            self.assertTrue(records[0]["stdin_tty"], records[0])
            self.assertTrue(records[0]["stdout_tty"], records[0])
            self.assertEqual(records[0]["input"], "KEYBOARD_INPUT_PROOF")
        finally:
            if not waited:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                os.waitpid(pid, 0)
            os.close(fd)

    def test_stop_targets_only_the_selected_private_core(self):
        result = subprocess.run(["sh", str(self.launcher), "--stop"],
                                env=self.env, capture_output=True, timeout=10, check=True)
        record = json.loads(result.stdout)
        self.assertEqual(record["args"], ["server", "stop"])
        self.assertEqual(record["socket"], str(self.home / "core.sock"))


if __name__ == "__main__":
    unittest.main()
