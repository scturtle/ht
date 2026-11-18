# herdr task runner (personal Core-only Linux build)

python := "python3"

# Fetch and verify the pinned libghostty-vt source archive (~64 MB extracted)
vendor-libghostty:
    {{python}} scripts/fetch_libghostty_vt.py

# Unit and contract tests (needs cargo-nextest, see README)
test:
    {{python}} tests/core-tui/test-harness.py
    cargo nextest run --locked --status-level fail --final-status-level fail --failure-output final --success-output never
    just arch-test
    just vendor-test

# Enforce deterministic UI hot-path architecture boundaries
arch-test:
    {{python}} -m unittest scripts.test_ui_hot_path_architecture

# Vendored patch index must stay accurate and still apply to the pinned tree
vendor-test:
    {{python}} -m unittest scripts.test_vendor_libghostty_vt

# Run one nextest filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final --success-output never

# Build the pinned local Core; never installs and never restarts a running Core.
build-core:
    {{python}} scripts/core-runtime/build.py build

# Core build contracts and the pinned Core's own test suite
test-core:
    {{python}} tests/core-runtime/test-build.py
    {{python}} scripts/core-runtime/build.py test

# Build the test-only ANSI oracle (no production dependency).
oracle:
    cargo build --locked --manifest-path tests/core-tui/Cargo.toml --bin screen-oracle

# Run all private Core/TUI smoke gates, or only the named cases (e.g. smoke keyboard copy).
[unix]
[positional-arguments]
smoke *cases: build build-core oracle
    CORE_TUI_HC="$PWD/.local/hc/bin/herdr-core" {{python}} tests/core-tui/smoke.py "$@"

# The launcher script must retain the caller's stdin and never retarget its Core.
[unix]
core-launcher-test:
    {{python}} tests/core-tui/test-launcher.py

# Formatting and clippy without tests
[unix]
lint:
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings

# Everything a pre-commit needs: lint, unit tests and contract tests
check: lint test

# Build the ht release binary (fetches the pinned libghostty-vt sources first)
[unix]
build: vendor-libghostty
    cargo build --release --locked

# Link the release binaries into ~/.local/bin so `ht` and `herdr-core` run from anywhere.
# Existing `hc` aliases point at the `herdr-core` link, so they follow the rebuild.
install: build build-core
    @mkdir -p ~/.local/bin
    @ln -sf "{{justfile_directory()}}/target/release/ht" ~/.local/bin/ht
    @ln -sf "{{justfile_directory()}}/.local/hc/bin/herdr-core" ~/.local/bin/herdr-core
    @echo "ht         -> {{justfile_directory()}}/target/release/ht"
    @echo "herdr-core -> {{justfile_directory()}}/.local/hc/bin/herdr-core"
    @echo "hc         -> herdr-core (existing alias)"

# Profile one visible pane as active-tab membership and background tabs scale
bench-render-scale:
    cargo test --release --locked --bin ht render_scale_profile -- --ignored --nocapture --test-threads=1

# Test-only stage clocks; no profiling checks or timers enter production rendering.
bench-render-stages:
    cargo test --release --locked --bin ht render_stage_profile -- --ignored --nocapture --test-threads=1

# Profile terminal target name resolution at increasing pane counts.
bench-terminal-targets:
    cargo test --release --locked --bin ht terminal_target_lookup_profile -- --ignored --nocapture --test-threads=1

# Profile full and retained text, static-image, and unchanged-image updates.
bench-retained-graphics:
    cargo test --release --locked --bin ht render_scale_profile_retained_graphics -- --ignored --nocapture --test-threads=1

# Profile first-batch latency and aggregate drain cost for external API bursts.
bench-api-fairness:
    cargo test --release --locked --bin ht external_api_burst_profile -- --ignored --nocapture --test-threads=1

# Print default config
default-config:
    cargo run --release --locked -- --default-config
