#!/usr/bin/env python3
"""Safety contracts; no Core or PTY is started."""
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import harness as H


class HarnessTest(unittest.TestCase):
    def test_inherited_selectors_and_injectors_cannot_escape_isolation(self):
        with patch.dict(os.environ, {"HC_SOCKET":"/human.sock","HC_SESSION":"default",
                "HERDR_SOCKET_PATH":"/human-legacy.sock","PI_SESSION":"private",
                "BASH_ENV":"/inject","SSH_TTY":"/dev/pts/1","HERDR_ENV":"1"}, clear=True):
            env=H.isolated_env("/var/tmp/owned","/var/tmp/owned/core")
        self.assertEqual(env["HERDR_ENV"],"1")
        for key in ("HC_SOCKET","HERDR_SOCKET_PATH","PI_SESSION","BASH_ENV","SSH_TTY"):
            self.assertNotIn(key,env)
        self.assertEqual(H.private_socket(env),Path(env["XDG_RUNTIME_DIR"])/"herdr-core"/(env["HC_SESSION"]+".sock"))
        self.assertIn("--core-session",H.argv(env))

    def test_stop_rejects_default_session_or_foreign_socket_before_rpc(self):
        env=H.isolated_env("/var/tmp/owned","/var/tmp/owned/core")
        for bad in (dict(env,HC_SESSION="default"),dict(env,HC_SOCKET="/human.sock")):
            with patch.object(H.subprocess,"run") as call:
                with self.assertRaises(AssertionError):H.stop_private_core(bad)
                call.assert_not_called()

    def test_absent_socket_does_not_start_a_core(self):
        env=H.isolated_env("/var/tmp/owned","/var/tmp/owned/core")
        with patch.object(Path,"exists",return_value=False),patch.object(H.subprocess,"run") as call:
            H.stop_private_core(env)
            call.assert_not_called()

    def test_client_argv_never_defaults_to_human_session(self):
        with self.assertRaises(KeyError):H.argv({})


if __name__=="__main__":
    unittest.main()
