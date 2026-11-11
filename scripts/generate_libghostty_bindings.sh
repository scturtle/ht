#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Additional clang arguments can supply a resource directory on minimal hosts.
"${BINDGEN:-bindgen}" .local/libghostty-vt/include/ghostty/vt.h \
  --allowlist-type 'Ghostty.*' \
  --allowlist-function 'ghostty_.*' \
  --allowlist-var 'GHOSTTY_.*' \
  --with-derive-default \
  --output crates/ghostty-vt/src/bindings.rs \
  -- -I.local/libghostty-vt/include "$@"
