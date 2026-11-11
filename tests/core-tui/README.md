# Core-backed TUI integration tests

These tests drive the real default Herdr client in `src/client/core/`,
`src/client/shell/` and `src/ui/`; this directory contains no alternative TUI.

The launcher and user instructions are in
[`scripts/core-tui/`](../../scripts/core-tui/README.md).
Pinned Core sources and patch documentation are in
[`scripts/core-runtime/`](../../scripts/core-runtime/README.md).

Run from the repository root:

```sh
just core-copy-smoke
just core-organization-smoke
just core-multiclient-smoke
just core-keyboard-smoke
just core-launcher-smoke
just core-zoom-smoke
just core-graphics-smoke
just core-reconnect-smoke
```

The recipes build Herdr, the pinned Core, and the test-only `screen-oracle`.
Its isolated Cargo package depends only on `vt100` and `serde_json`; it decodes
captured PTY output, including host resize boundaries. It owns no application
PTY, Core connection or runtime state.

The Unix tests use private HOME/config/runtime/socket/PTY directories and clean
up only their own processes. Desktop clipboard access is disabled; OSC 52 is
captured instead. None of the individual gates constitutes full migration or
server-handoff acceptance.

The client attaches with `terminal.attach_ordered`: every first and replacement
paint is preceded by an authoritative RESIZE. There is no paint-completion
marker, so replay bytes and live bytes are not distinguished and semantic input
is not gated on a boundary; unit coverage for the frame contract lives in
`src/client/core` instead of a relay fixture.

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

`multiclient-ui-smoke.py` is the gate for the `latest` geometry policy: clients
that disagree on a pane's size converge on the last interacting client's grid,
the other side follows without misparsing and can reclaim by focusing the pane.
The retired paint/cells fixtures are kept under `.local/core-only/retired-tests/`.

`existing-ui-smoke.py` supplies the shared PTY driver. Its old milestone main
function is historical and is not a current gate; use the recipes above.
