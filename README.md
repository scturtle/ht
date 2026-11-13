# ht

A personal, Linux-only fork of [Herdr](https://github.com/herdrdev/herdr), using
**herdr-core as its only runtime** and reusing the original terminal UI.

- Workspaces, tabs, split panes, keyboard and mouse controls.
- Multiple clients share running programs while keeping independent focus.
- The last interacting client controls a pane's size; other clients follow it.
- Detaching leaves programs running. Copy, search and local diagnostics are built in.

There is no legacy server, federation, release channel or upstream merge path.
`ht` is the UI client; `hc` is the Core CLI, not an agent-facing `ht` API.

## Build and run

Requires the pinned Rust toolchain, a C toolchain, Python 3, `just` and libclang.
The client uses Zig 0.17; Core uses Zig 0.16.0. Select them with `ZIG` and
`HERDR_CORE_SOURCE_ZIG`, respectively.

```sh
just build                 # fetch verified libghostty-vt sources and build ht
just core-source-build     # build the pinned, locally patched Core

HERDR_CORE_BIN="$PWD/.local/core-runtime/target/release/herdr-core" \
  ./target/release/ht
```

With the binaries on `PATH`, launch with `ht`. For a separate session:

```sh
env -u HC_SOCKET ht --core-home /path/to/private-home --core-session work
```

A different home alone does not isolate a session. Building does not restart or
upgrade a running Core. See [Core runtime notes](scripts/core-runtime/README.md)
for source pins, local patches and upgrade boundaries.

## Navigation

There is no persistent sidebar. Open the workspace chooser with **Ctrl+b, then w**
(default `prefix+w`). Use arrows and Enter to select, Esc to cancel.
**Ctrl+b, then g** opens the session navigator, with arrows or j/k to select.
The chooser has no menu section; the navigator has no search or filters.
Help remains available via **prefix+?**; pane diagnostics are in the pane context menu.

## Configuration

Edit `~/.config/herdr/config.toml` (or `HERDR_CONFIG_PATH`); there is no settings page.
Use `ht --default-config` for the supported options. UI colors follow the host terminal,
with one optional override layer:

```toml
[theme.custom]
accent = "#12ab34"
panel_bg = "reset"
```

There are no built-in themes, light/dark overrides, auto-switching or audio alerts.
**prefix+Shift+r** reloads live client options; startup-only options require reattaching.

## Development

```sh
just test                  # unit and contract tests; requires cargo-nextest
just check                 # formatting, clippy and tests
just core-multiclient-smoke # real TUI against a private Core
```

Read [AGENTS.md](AGENTS.md) before changing code. Local planning notes belong in
`.local/prd/`.

## License

[Apache License 2.0](LICENSE). Based on Herdr by its original contributors.
