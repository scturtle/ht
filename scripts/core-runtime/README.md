# Pinned local Core runtime

This personal-fork runtime extends **Core `f0828f1e40fac656a5bac077e20c605d38760164`**
(post-0.1.20) without reinstating Herdr's retired PTY server. It is not an upstream
release or an installation/update tool. The upstream package version still says
`0.1.20`; use `source.json` and the running ELF build identity, not that version
number alone, to distinguish builds.

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

Downloaded archives are keyed by their pinned SHA-256, so upgrading a pin keeps
previous downloads and prepared bundles available rather than replacing them.
A matching historical unkeyed archive is reused only after digest verification;
a corrupt keyed archive is refused, not overwritten.

Native artifacts must not be shared across different bundles: Cargo's build
script cache can otherwise retain the previous bundle's Ghostty static library.
A failed build never replaces the published executable. No command installs to
PATH, changes systemd/linger, restarts an existing server, or migrates a session.
Tests use their own disposable runtimes.

## Current upstream update

The full Core source pin, not just the query-reply changes, moved from `de97ea5`
to `f0828f1`. Ghostty's submodule pin is unchanged. This includes the upstream
`1c22a51` / `f0828f1` image-answer/DA1 ordering fixes, hidden-primary-screen replay,
waiter cleanup, and Linux login-environment fixes. The original three local
patches are rebased on that source; patch `0006` adds automatic image-answer
arbitration on top of the upstream ordering fix.

Upstream holds only DA1 behind a complete correlated viewer image answer,
with a default 100 ms deadline and one shared timer task. Alone, it still permits
`image answer → DA1 → second image answer` with two viewers, and a probe answer
after DA1 when the viewer takes longer than the deadline. Patch `0006` consumes
these correlated duplicate/expired automatic answers. Cursor/status replies
remain immediate; normal image-upload ACKs are not expired with capability probes.

Building atomically replaces the executable used for **new** Core processes; it
does not replace a running Core. A fresh ht connection to an existing socket also
keeps using that existing Core. Test with an unused `--core-home` **and** a private
`--core-session` (unset an inherited `HC_SOCKET`), or arrange an explicit manual
handover for your actual session. No build/test command performs that handover.

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
inactive-screen transitions and binary snapshot round trips. With the current
upstream hidden-primary replay, the alternate stack is restored after its DECSTR,
and tests compare both screens' contents as well as later keyboard pops. Existing
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

### 0006 — single-winner automatic image answers

Base: Core `f0828f1e40fac656a5bac077e20c605d38760164`, after patches `0001`–`0003`.
Touches `src/relay.rs`, `src/relay/graphics.rs`, `src/viewers.rs`, runtime tests,
and `tests/viewer_activity.rs` in `crates/herdr-core`.

One queried program has one reply stream even when several ht clients view it.
The first complete correlated `AUTOMATIC_INPUT` image reply wins per query
sequence number, not permanently per image ID. Other viewers' matching replies
are consumed. An unanswered `a=q` capability probe retires when its following
DA1 barrier is released, so its late reply cannot escape into Emacs's command
input. A probe without a following DA1, or after an earlier DA1 in the same output
chunk, stays answerable beyond the timer. Real upload replies also remain
eligible for their first ACK after that deadline.

Only explicit automatic input is framed/filtered. Fragmented graphics responses
use at most 512 retained bytes per viewer; oversized/unknown replies pass through
without guessed correlation. Legacy mixed DATA remains byte-transparent, so this
is not a deduplication guarantee for clients sending automatic replies as DATA.
Human keys/Escape/paste are never held by an unfinished automatic response and
automatic replies still cannot claim focus, size or display leadership. Existing
per-viewer query histories and new single-winner history are bounded at 4096;
a viewer whose correlation history saturates cannot answer further automatic
image queries until reattached. Other replies and human DATA remain unaffected.
No wire kind, PTY input gate, timer task, image payload or renderer is added.

Verification: a deterministic two-viewer regression fails on the old reducer;
new tests cover both winner orders, all response splits, interleaved human keys,
probe expiry, delayed old replies after ID reuse, late real-upload ACKs, absent/
preceding/following DA1 barriers including one-chunk boundaries, unknown and
malformed replies, bounded framing/history, paste and unchanged leadership.
`just core-image-answers-smoke` uses the actual ht/Core/PTY and Emacs's PNG probe,
including a deliberately delayed **owned** client. It requires no model or Emacs
installation. Full Core tests and existing graphics/keyboard/multiclient gates
must continue to pass.

Removal condition: the pinned Core offers equivalent single-winner automatic
reply and late-probe retirement semantics, and these tests pass without it.
This does not alter Emacs's own query timeout or promise arbitrary WAN latency.

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

The current `build.py prepare` applies `0001`, `0002`, `0003`, then the independent
`0006` image-answer patch with zero fuzz, and the prepared `hello` advertises
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
