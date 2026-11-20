# Core-backed TUI integration tests

These tests drive the real default Herdr client in `src/client/core/`,
`src/client/shell/` and `src/ui/`; this directory contains no alternative TUI.

The launcher and user instructions are in
[`scripts/core-tui/`](../../scripts/core-tui/README.md).
Pinned Core sources and patch documentation are in
[`scripts/core-runtime/`](../../scripts/core-runtime/README.md).

Run from the repository root:

```sh
just smoke                # all smoke gates, serially
just smoke keyboard copy  # selected gates only
just core-launcher-test
```

`just smoke organization` also checks direct pane selection in wide and narrow
clients sharing the same tab: the device-local focus changes without modifying
the shared layout or restarting either program.

`just smoke app-layout` starts `ht` with its default socket and layout selectors
in a private runtime, with an App-format fixture writer and real Core PTYs.
It checks both startup orders and that the App fixture can read the layout
and the first shell created by `ht`. It also checks Tab creation, rename and keyboard/mouse reorder, Space creation,
rename and close, Tab number vs Pane cycling, independent focus, and that
scoped edits preserve unrelated PTYs and plugin data.
It does not launch the desktop GUI or a native mobile app.

Add new gates as `<name>-ui-smoke.py`; the runner discovers them automatically and
returns a nonzero status if any gate fails. The compatibility alias `existing-ui-smoke.py`
is not a separate gate.

The smoke recipe builds Herdr, the pinned Core, and the test-only `screen-oracle`.
Its isolated Cargo package depends only on `vt100` and `serde_json`; it decodes
captured PTY output, including host resize boundaries. It owns no application
PTY, Core connection or runtime state.

The Unix tests use private HOME/config/runtime/socket/PTY directories and clean
up only their own processes. Desktop clipboard access is disabled; OSC 52 is
captured instead.

The client attaches with `terminal.attach_ordered`: every first and replacement
paint is preceded by an authoritative RESIZE. There is no paint-completion
marker, so replay bytes and live bytes are not distinguished and semantic input
is not gated on a boundary; unit coverage for the frame contract lives in
`src/client/core` instead of a relay fixture.

`terminal-restore-ui-smoke.py` checks real TUI handoff to an outer shell on a
private PTY that never answers color queries. Detach, SIGINT/SIGTERM/SIGHUP, and
Ctrl+D closing the last pane must exit without waiting for replies or sending
a handoff query. The gate checks that the tty settings are restored, input
reporting and the alternate screen are off, and the first shell command is
preserved. It also checks that colors are queried only at startup and on
manual config reload.

`kitty-exit-ui-smoke.py` is the separate **actual Kitty + SSH** gate. Run
`just smoke kitty-exit`; it requires Kitty/kitten, Xvfb, libX11/libXtst and
OpenSSH server/client. `CORE_TUI_KITTY`, `CORE_TUI_KITTEN` and `CORE_TUI_XVFB`
override executable paths. It creates its own virtual X display and loopback
SSH listener/keys, never connects to the user's display, SSH service or Core.
A TCP proxy adds 300ms each way by default (600ms RTT;
`CORE_TUI_SSH_DELAY_SECONDS` overrides the per-direction delay). Twelve Ctrl+D
exits (`CORE_TUI_KITTY_EXIT_RUNS`) vary early/normal timing and continuous motion,
wheel or both. Each run checks the exit code and restored tty settings. A
post-exit raw receiver records later input for 1.5 seconds without flushing;
in-flight terminal reports can arrive after handoff, so the capture is
diagnostic rather than required to be empty. Owned processes/Core are stopped
in `finally`; logs and captures stay in the printed `/var/tmp/ht-kitty-*`
directory. This is loopback SSH under controlled latency.

`reconnect-ui-smoke.py` is the reconnect gate. `server restart` must not end the
TUI: the dead relay is replaced, panes re-attach, screens come back from Core's
preserved state and typed input reaches the same program. `server stop` must
never be undone: no socket reappears and no process is spawned. It uses the
planned restart rather than SIGKILL because this environment has no holder
(`holders: []`), so a crashed Core has nothing to adopt the programs.

`graphics-ui-smoke.py` is the Kitty graphics gate: the host terminal must receive
the image transmission and its placement, a metadata-only update must not delete
the image, and the same image is never re-transmitted. Its host PTY reports cell
pixels (8x16), because a host that reports none has no measurable placement and
images are correctly skipped.

`icat-ui-smoke.py` adds the real application's prerequisite: `TIOCGWINSZ`
must report the pane's pixel dimensions, and ordinary `kitten icat` (without
`--use-window-size`) must exit successfully and emit an upload and placement.
Run `just smoke icat`; requires kitten (`CORE_TUI_KITTEN` overrides its path).
For a specific image, run `python3 tests/core-tui/icat-ui-smoke.py --image /path/to/image.png`.
Like the graphics gate it uses only a private Core and a pixel-aware host PTY.

`multiclient-ui-smoke.py` is the gate for the `latest` geometry policy: clients
that disagree on a pane's size converge on the last interacting client's grid,
the other side follows without misparsing and can reclaim by focusing the pane.
The retired paint/cells fixtures were removed; the recipes above are the current gates.

`existing-ui-smoke.py` supplies the shared PTY driver. Its old milestone main
function is historical and is not a current gate; use the recipes above.
