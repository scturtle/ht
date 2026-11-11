# herdr task runner (personal Core-only Linux build)

python := "python3"

# Fetch and verify the pinned libghostty-vt source archive (~64 MB extracted)
vendor-libghostty:
    {{python}} scripts/fetch_libghostty_vt.py

# Run tests
test: vendor-libghostty
    cargo nextest run --locked --status-level fail --final-status-level fail --failure-output final --success-output never
    just ui-hot-path-architecture-test
    just vendor-patches-test

# Enforce deterministic UI hot-path architecture boundaries
ui-hot-path-architecture-test:
    {{python}} -m unittest scripts.test_ui_hot_path_architecture

# Vendored patch index must stay accurate and still apply to the pinned tree
vendor-patches-test:
    {{python}} -m unittest scripts.test_vendor_libghostty_vt

# Run one nextest filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final --success-output never

# Pinned local Core extension, never installed and never restarts another Core.
core-source-build:
    {{python}} scripts/core-runtime/build.py build

core-source-test:
    {{python}} tests/core-runtime/test-build.py
    {{python}} scripts/core-runtime/build.py test

# Build the test-only ANSI oracle (no production dependency).
core-ui-oracle:
    cargo build --locked --manifest-path tests/core-tui/Cargo.toml --bin screen-oracle

# Focused real-TUI copy-mode gate; not full Core-only migration acceptance.
[unix]
core-copy-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/copy-ui-smoke.py

# Real-TUI shared organization/CAS lifecycle; not full migration acceptance.
[unix]
core-organization-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/organization-ui-smoke.py

# Two real clients, authoritative resize ordering, leadership and copy-mode.
[unix]
core-multiclient-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/multiclient-ui-smoke.py

# Semantic Kitty input and stack replay across intentional client replacement.
[unix]
core-keyboard-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/keyboard-ui-smoke.py

# The actual user launcher must retain stdin and deliver real keyboard input.
[unix]
core-launcher-smoke: build core-source-build core-ui-oracle
    {{python}} tests/core-tui/test-launcher.py
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/launcher-ui-smoke.py

# A lost Core connection must recover; an explicit stop must never be undone.
[unix]
core-reconnect-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/reconnect-ui-smoke.py

# A program's Kitty image must reach the host terminal through the real TUI.
[unix]
core-graphics-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/graphics-ui-smoke.py

# Original zoom behavior through the actual launcher and two isolated clients.
core-zoom-smoke: build core-source-build core-ui-oracle
    CORE_TUI_HC="$PWD/.local/core-runtime/target/release/herdr-core" {{python}} tests/core-tui/zoom-ui-smoke.py

# Run fast local lint checks
[unix]
lint: vendor-libghostty
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings

# Run local checks: lint + unit tests + maintenance contract tests
ci: vendor-libghostty lint
    just test

check: vendor-libghostty ci

# Install repo-local git hooks
install-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/pre-commit
    chmod +x .githooks/commit-msg
    @echo "installed git hooks from .githooks"

# Build release binary
[unix]
build: vendor-libghostty
    cargo build --release --locked

# Symlink the release binaries into ~/.local/bin so `ht` and `hc` run them from anywhere.
# `hc` is the existing alias for `herdr-core`, so replacing that link is enough.
install: build core-source-build
    @mkdir -p ~/.local/bin
    @ln -sf "{{justfile_directory()}}/target/release/ht" ~/.local/bin/ht
    @ln -sf "{{justfile_directory()}}/.local/core-runtime/target/release/herdr-core" ~/.local/bin/herdr-core
    @echo "ht         -> {{justfile_directory()}}/target/release/ht"
    @echo "herdr-core -> {{justfile_directory()}}/.local/core-runtime/target/release/herdr-core"
    @echo "hc         -> herdr-core (existing alias)"

# Non-gating full-render scaling profile for background workspaces and active panes
bench-render-scale:
    cargo test --release --locked --bin ht render_scale_profile -- --ignored --nocapture --test-threads=1

# Profile terminal target name resolution at increasing pane counts.
bench-terminal-targets:
    cargo test --release --locked --bin ht terminal_target_lookup_profile -- --ignored --nocapture --test-threads=1

# Profile BSP split collection and construction with balanced and skewed trees.
bench-bsp-layout:
    cargo test --release --locked --bin ht bsp_layout_profile -- --ignored --nocapture --test-threads=1

# Profile full and retained text, static-image, and unchanged-image updates.
bench-retained-graphics:
    cargo test --release --locked --bin ht render_scale_profile_retained_graphics -- --ignored --nocapture --test-threads=1

# Profile first-batch latency and aggregate drain cost for external API bursts.
bench-api-fairness:
    cargo test --release --locked --bin ht external_api_burst_profile -- --ignored --nocapture --test-threads=1

# Print default config
default-config:
    cargo run --release --locked -- --default-config
