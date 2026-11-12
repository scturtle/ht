# Pinned Core builder contracts

`just core-source-test` first runs `test-build.py`, then the pinned Core's own
full Rust suite. The Python tests use tiny local `file://` archives under a
private `/var/tmp` root; they download no network sources, start no Core, and
never use the real build cache or user runtime.

They check that a fresh `prepare` prints only the verified source path on stdout
(progress goes to stderr), cached preparation is stable, and edited prepared
sources are refused without discarding the edits. Archive caches are digest-keyed:
upgrading a source pin preserves its previous downloads and prepared sources,
matching legacy downloads are reused, and corrupt keyed downloads are refused
without being overwritten.

Protocol, paint-boundary, real-PTY and schema tests live in the pinned Core source
patches under [`scripts/core-runtime/`](../../scripts/core-runtime/README.md).
The real default-TUI gates are under [`tests/core-tui/`](../core-tui/README.md).
