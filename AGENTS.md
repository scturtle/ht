# herdr (personal Core-only fork)

A personal Linux-only fork of Herdr where **`herdr-core` is the only runtime**.
There is no private Herdr server, no federation, no release channel, and no
upstream merge path. This file describes what still matters when changing code.

## Runtime shape

- Core owns PTYs, screens and the shared session document. The client owns
  rendering, input translation, focus, zoom and presentation preferences.
- `src/client/core/` is the adapter: one `hc connect` relay per client, ordered
  attach per pane, snapshot + surface frames into the original shell UI.
- The original UI in `src/client/shell/` and `src/ui/` is reused unchanged. Do
  not add a second chrome renderer.
- Only advertise what the adapter actually implements
  (`src/client/core/methods.rs`).
- Wire kinds: DATA=0, CREDIT=1, CLOSE=2, RESIZE=3, FOCUS=4, FACTS=5,
  AUTOMATIC_INPUT=6. Ordered attach sends an authoritative RESIZE before every
  first/replacement paint.
- Geometry policy is `latest`: one PTY has one size, so the last interacting
  client owns a pane's grid and others follow it.
- Killing the relay must never stop Core. Dropping the client leaves holders
  and programs alive.

## Principles

- **State is separated from runtime.** `Model`/`Organization` are pure data,
  testable without PTYs or async.
- **Render is pure.** Composition handles geometry; drawing never mutates state.
- **No god objects.** Keep `client/core` split by concern.
- **Platform code lives in `src/platform/`.** This fork is Linux-only: there is
  no `#[cfg(windows)]` or `#[cfg(target_os = "macos")]` code, and no Windows or
  macOS build. Add platform behavior in `src/platform/linux.rs` or
  `src/platform/unix_common.rs`.

## Multiplicative performance paths

Treat work reachable from view computation, rendering, PTY parsing, detection
and frame fanout as multiplicative: identify the frequency (per byte, event, or
render) and the cardinality (panes, tabs, workspaces) before adding work.

Inside pane-scaled loops: use narrow terminal accessors, never collect
aggregate input state, format snapshots, inspect process trees, or allocate when
one scalar fact is enough. Keep terminal-core lock duration minimal, and preserve
hidden-pane/retained-render early exits.

`just bench-render-scale` profiles fixed geometry at 1 and 15+ panes when a
change widens work in one of these loops.

## Vendored libghostty-vt

`libghostty-vt` is not committed. `vendor/libghostty-vt.vendor.json` pins the
`vt-1.0.0` tag archive URL and its SHA-256, plus the paths the build extracts so
macOS assets, docs and tests never enter the tree. Fetch it once per checkout:

```bash
just vendor-libghostty      # = python3 scripts/fetch_libghostty_vt.py
```

That verifies the digest and extracts to `.local/libghostty-vt` (~64 MB). The
build fails with that instruction when the tree is missing.

`crates/ghostty-vt/build.rs` stages a copy under `OUT_DIR`, writes the
`VERSION`/`LIB_VERSION` files the tag does not ship, and applies the local
patches there before `zig build`, so the fetched tree stays pristine. Staging
is keyed by the vendor identity, verified source stamp, selected source contents,
patches and staging rules. The tag in the vendor manifest supplies all three build
version strings; do not add another hardcoded version. The cache identity rules
have direct tests in `crates/ghostty-vt/build_support.rs`.

The C bindings are generated into `OUT_DIR` from those patched headers by
`bindgen`, not committed. That is deliberate: a patch that changes the C API
would otherwise leave a stale binding that compiles cleanly and is silently
unsound. Binding generation runs libclang, so a build also needs a libclang shared
library on the machine.

The pinned URL uses the tag form (`/archive/refs/tags/<tag>.tar.gz`). The
commit form (`codeload.../tar.gz/<commit>`) is not byte-reproducible: two
downloads of one commit returned identical content but different bytes, which
would make a digest pin useless.

Local patches live under `vendor/patches/libghostty-vt/` and must be indexed in
`vendor/libghostty-vt.patches.md` with the reason, base commit, touched files,
verification and removal condition. `just vendor-patches-test` enforces that every
patch file is indexed and still applies to the pinned tree. Never leave an
untracked patch or an indexed patch that no longer applies.

Building requires Zig 0.17.0. The pinned Core source bundle under
`scripts/core-runtime/` still vendors a pre-1.0 libghostty snapshot and needs Zig
0.16.0; `scripts/core-runtime/build.py` pins that toolchain separately through
`HERDR_CORE_SOURCE_ZIG` so one `ZIG` export cannot serve both builds.

## Testing

```bash
just test     # cargo nextest + architecture/vendor contract tests
just check    # fmt + clippy -D warnings + the above
```

Real-TUI gates drive the actual launcher against a private Core/PTY:

```bash
just core-multiclient-smoke
just core-organization-smoke  just core-copy-smoke
just core-keyboard-smoke      just core-graphics-smoke
just core-reconnect-smoke     just core-launcher-test
just core-diagnostics-smoke    just core-scheduling-smoke
```

Build the pinned Core runtime with `just core-source-build`; `source.json` lists
the three applied patches, documented in `scripts/core-runtime/README.md`.

Unit tests live next to the code (`#[cfg(test)] mod tests`). New adapter
behavior should be testable with `Model::new(...)` and `Organization` without
PTYs. Use `just test-one <filter>` for a single test.

When testing a new build from inside an existing Herdr session, clear inherited
overrides so the debug binary talks to its own server:

```bash
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH cargo run -- <command>
```

## Code conventions

- No `unwrap()` in production code; use `tracing` for logging; `#[allow]` only
  with a comment explaining why.
- Don't add dependencies without a reason; check whether an existing one covers
  the need.
- Commit style: lowercase conventional commits, no emojis, no AI co-author
  lines. This fork is local-only: do not push, tag, or open pull requests.

## Local notes

- Local PRDs and planning notes go under `.local/prd/` (ignored by git).
- The CLI surface is intentionally tiny: `--core`, `--version`, `--default-config`,
  `--help`, and `config …`. There is no agent-facing API or CLI bridge.
