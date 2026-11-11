# Pinned local Core runtime

This personal-fork runtime extends **Core 0.1.20** without reinstating Herdr's
retired PTY server. It is not an upstream release or an installation/update tool.

```sh
just core-source-build
just core-source-test
```

`source.json` pins both Core and its Ghostty submodule by commit, archive URL and
SHA-256. `build.py` checks downloads, applies the listed patches with zero fuzz,
and verifies a complete source digest before and after building/testing. It
refuses to replace edited prepared sources. Inputs are source-reproducible;
binaries are not promised bit-identical across different toolchains. Use the
project's Rust toolchain and Zig 0.16.0.

- Prepared sources: `.local/core-runtime/prepared/<bundle-id>/`
- Per-bundle Cargo/native artifacts: `.local/core-runtime/artifacts/<bundle-id>/`
- Atomically published executable: `.local/core-runtime/target/release/herdr-core`

Native artifacts must not be shared across different bundles: Cargo's build
script cache can otherwise retain the previous bundle's Ghostty static library.
A failed build never replaces the published executable. No command installs to
PATH, changes systemd/linger, restarts an existing server, or migrates a session.
Tests use their own disposable runtimes.

## Local patch inventory

These are personal-fork Core patches, not modifications to Herdr's separately
vendored `vendor/libghostty-vt` tree. No upstream issue or PR has been submitted.

### 0001 — ordered interactive attach

Adds the advertised `ordered_attach` capability and `terminal.attach_ordered`
method; preserves old `terminal.attach` parameters and behavior. The new method
orders initial and replacement **RESIZE → paint → live DATA**, including
backpressured/coalesced geometry. A resumed hidden viewer is passive until
human input, explicit focus or resize activates it. Automatic query replies do
not claim leadership. Remote forwarding rejects the extension explicitly.

Verification: ordered viewer tests, legacy compatibility tests, schema fixture,
full Core suite, and two real default Herdr TUIs with different geometries,
shared tabs, client-local focus, background views and copy/search.

Removal condition: the pinned upstream Core provides and advertises an equivalent
ordered interactive attach and passive-viewer contract, and all these tests pass
against it. Never remove this patch merely because an observer resize event exists.

### 0002 — complete Kitty keyboard flag-stack replay

Adds `keyboard_stacks_v1`. A new versioned, read-only Ghostty C API copies the
primary and alternate screen's bounded flag rings without cloning or mutating
the terminal. Core's VT paint restores both rings, including overflow slots and
dirty-viewer state, before painting the active screen. Temporary screen switches
leave the original canonical alternate-screen mode contract intact. Neither a
new frozen Herdr codec nor an old endpoint method shape is changed.

Verification: a failing characterization first demonstrated dirty-viewer flags
leaking after a pop. New tests cover fresh/dirty viewers, overflow, later pops,
inactive-screen transitions and binary snapshot round trips. Existing
screen/cursor/mode tests pass. `just core-keyboard-smoke` verifies real semantic
press/repeat/release, client replacement, nested pops and eventual legacy input.

Removal condition: the pinned Core/Ghostty offers equivalent complete bounded
stack replay for both screens and advertises the capability; the characterization
and real-TUI tests must pass without this patch.

### 0003 — non-starting connection probes

Advertises `existing_connect` and adds `hc connect --existing`, which reaches a
running Core and never starts or replaces one. Plain `hc connect` keeps its
auto-start behavior for explicit launches.

The client's reconnect path uses it: after a transport failure Herdr retries the
same Core home, and a probe that auto-started would undo an explicit
`server stop` and silently resurrect a runtime the user shut down. The patch's
own test (`existing_connect_never_starts_or_resurrects_a_stopped_core`) is the
behavioral contract: no socket is created for a missing Core, and a probe after
`server stop` leaves no socket behind.

Retired on 2026-11-10 because nothing consumed it; reintroduced on the same day
together with the reconnect state machine that does.

Verification: the patch test above, the full Core suite, and
`just core-reconnect-smoke`, which stops a private Core and asserts that no
socket reappears.

Removal condition: the pinned upstream Core advertises `existing_connect` with
the same never-start semantics, and the reconnect path keeps passing.

### Retired on 2026-11-10 (B+ cutover)

Two patches are still dropped from the bundle; their files are kept for reference
under `.local/core-only/retired-patches/` and are never applied again.

- **0004 — paint-completion boundaries** (`paint_barriers_v1`,
  `terminal.attach_painted`). The client accepted two consequences instead:
  canonical replay is not distinguished from live bytes, and semantic input is
  not gated on a completion marker. Ordered attach already guarantees the
  authoritative RESIZE before every paint, which is what multi-client
  correctness needs; a completion marker only helped suppress replay effects
  and hold input.
- **0005 — cells/semantic keys** (`terminal_cells_v1`, `semantic_keys_v1`).
  A pull-only feasibility proof that the running TUI never consumed. It also
  added three undocumented methods to a live Core. `herdr-app` shows a client
  VT over plain attach is viable without it.

Evidence that the reduced bundle is sufficient: `build.py prepare` applies
`0001` then `0002` with zero fuzz, and the prepared `hello` advertises exactly
`ordered_attach` + `keyboard_stacks_v1` with no attach_painted/attach_cells.

## Geometry policy: `latest`

The client uses `terminal.attach_ordered` and sends its own RESIZE when it
takes focus or reveals a pane, so the last interacting client owns a pane's
grid. Other clients are told the new grid and follow it, which keeps every
parser correct; a client whose layout is smaller than the owner's grid shows
that pane clipped rather than wrongly rewrapped. Both clients cannot each get
their own PTY size: one PTY has one size.

## Not provided by these extensions

- Acknowledgement that the outer host has actually drawn a frame.
- Core-owned image scenes and complete Kitty graphics paint/recovery.
- Epoch fencing for reconnect: Core exposes a permanent state id, not a boot
  epoch, so the client cannot prove "this input was queued for the previous
  Core". Reconnect discards queued input instead of replaying it.
- Full keyboard text/AltGr/dead-key and graphics/handoff joint acceptance.
- Remote Core transport or automated Herdr CLI bridge.

Do not infer image/screen recovery from a preserved PID or from binary keyboard
snapshot tests. The overall migration gate remains open.
