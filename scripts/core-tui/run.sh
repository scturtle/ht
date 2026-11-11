#!/bin/sh
# Launch the default Core-backed Herdr TUI with an isolated runtime.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
export PATH="$ROOT/.local/toolchain:$PATH"
# Herdr's vendored libghostty-vt 1.0 needs Zig 0.17; the pinned Core source
# bundle pins its own (0.16) toolchain inside scripts/core-runtime/build.py.
if [ -z "${ZIG:-}" ] && [ -x "$ROOT/.local/toolchain/zig-x86_64-linux-0.17.1/zig" ]; then
    export ZIG="$ROOT/.local/toolchain/zig-x86_64-linux-0.17.1/zig"
fi
if [ "${1:-}" != "--stop" ]; then
    if [ -z "${CORE_TUI_HC:-}" ]; then
        (cd "$ROOT" && just core-source-build)
    fi
    (cd "$ROOT" && just build)
fi
# Keep stdin attached to the caller: a Python heredoc here would consume it.
exec python3 "$ROOT/scripts/core-tui/run.py" "$ROOT" "$@"
