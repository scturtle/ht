# ht

A personal, Linux-only fork of [Herdr](https://github.com/herdrdev/herdr), using
**herdr-core as its only runtime** and reusing the original terminal UI.

- Workspaces, tabs, split panes, keyboard and mouse controls.
- Multiple clients share running programs while keeping independent focus.
- The last interacting client controls a pane's size; other clients follow it.
- Detaching leaves programs running. Copy, search and local diagnostics are built in.

## Build

A working installation needs **two binaries**, built independently:

| Component | Sources | Zig | Output |
|---|---|---|---|
| Client (`ht`) | this workspace + libghostty-vt `vt-1.0.0` | 0.17.x | `target/release/ht` |
| Core (`hc`) | herdr-core | 0.16.0 | `.local/hc/bin/herdr-core` |

### 1. Prepare dependencies

Install Rust 1.100.0 with clippy and rustfmt through
[rustup](https://rustup.rs/) (`rust-toolchain.toml` pins the channel), a libclang
shared library for bindgen (`LIBCLANG_PATH` if it is not auto-detected), and
`just`, which drives every build recipe, plus `cargo-nextest` for `just test`,
with `cargo install` into `~/.local/bin` kept on `PATH`.

### 2. Prepare the two Zig toolchains

The client uses Zig 0.17.x and Core uses Zig 0.16.0, because they vendor
different Ghostty sources; the newer Zig cannot build Core, whose embedded
Ghostty rejects any other minor version. Install both side by side as
`.local/toolchain/zig/<version>/`, then point `ZIG` at the client one. Core's
build ignores `ZIG` and always uses `.local/toolchain/zig/0.16.0/zig`, failing
with a clear message when that file is missing.

### 3. Build the client and Core

```sh
just build        # client: fetch verified libghostty-vt sources, then build ht
just build-core   # Core: fetch, patch, build and atomically publish herdr-core
```

### 4. Install (optional)

`just install` builds both components and symlinks `ht` and `herdr-core` into
`~/.local/bin`.

### Working directories

Keep `toolchain/` (tools plus `zig/<version>/`), `core-tui/` (trial runtime home
and personal config). Everything else rebuilds on demand: `cache/` and
`libghostty-vt/` for the client, `hc/` for Core (`downloads/`, `work/` and the
published `hc/bin/herdr-core`). Client Cargo outputs stay in `target/`.

## Run

```sh
HERDR_CORE_BIN="$PWD/.local/hc/bin/herdr-core" ./target/release/ht
```

With the binaries on `PATH` (via `just install`), launch with `ht`.

## Configuration

Edit `~/.config/herdr/config.toml` (or `HERDR_CONFIG_PATH`).

## License

[Apache License 2.0](LICENSE). Based on Herdr by its original contributors.
