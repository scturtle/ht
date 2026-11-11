#!/usr/bin/env python3
"""Select the isolated runtime without redirecting the TUI\'s standard input."""
import os
from pathlib import Path
import sys
root = Path(sys.argv[1])
hc = Path(os.environ.get('CORE_TUI_HC', root / '.local/core-runtime/target/release/herdr-core')).resolve()
home = Path(os.environ.get('CORE_UI_HOME', root / '.local/core-tui/existing')).resolve()
config = os.environ.get('CORE_UI_CONFIG')
env = {k:v for k,v in os.environ.items() if not k.startswith(('HERDR_', 'HC_'))}
if not hc.is_file():
    sys.exit(f'Core missing: {hc}. Build it with just core-source-build, or set CORE_TUI_HC to a compatible patched binary.')
if sys.argv[2:] == ['--stop']:
    env.update(HC_SESSION='herdr-tui', HC_SOCKET=str(home / 'core.sock'),
               XDG_RUNTIME_DIR=str(home / 'run'), XDG_STATE_HOME=str(home / 'state'),
               XDG_CONFIG_HOME=str(home / 'config'))
    os.execve(hc, [str(hc), 'server', 'stop'], env)
if sys.argv[2:]:
    sys.exit('usage: run.sh [--stop]')
env.update(HERDR_CORE_BIN=str(hc), HERDR_CORE_HOME=str(home))
# Opt-in replacement identity; never inherit an outer TUI's HERDR_* identity.
if 'CORE_UI_CLIENT_ID' in os.environ:
    env['HERDR_CORE_CLIENT_ID'] = os.environ['CORE_UI_CLIENT_ID']
if config:
    env['HERDR_CONFIG_PATH'] = str(Path(config).resolve())
print(f'Core-backed Herdr TUI; isolated runtime: {home}', flush=True)
if config:
    print(f'UI configuration: {env["HERDR_CONFIG_PATH"]}', flush=True)
else:
    print('Uses your normal Herdr configuration, including custom keybindings.', flush=True)
if config and Path(config).resolve() == (root / 'scripts/core-tui/try-config.toml').resolve():
    print('Trial keys: Ctrl+b or F12 prefix; F2/F3 split; F6 next pane; F10 detach.', flush=True)
else:
    print('An outer Herdr/tmux may capture the same prefix. Optional trial config: scripts/core-tui/try-config.toml', flush=True)
os.execve(root / 'target/release/ht', ['ht'], env)
