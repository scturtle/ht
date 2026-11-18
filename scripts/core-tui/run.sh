#!/bin/sh
# Launch the default Core-backed Herdr TUI with an isolated runtime.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
export PATH="$ROOT/.local/toolchain:$PATH"
if [ "${1:-}" != "--stop" ]; then
    if [ -z "${CORE_TUI_HC:-}" ]; then
        (cd "$ROOT" && just build-core)
    fi
    (cd "$ROOT" && just build)
fi
# Keep stdin attached to the caller: a Python heredoc here would consume it.
exec python3 "$ROOT/scripts/core-tui/run.py" "$ROOT" "$@"
