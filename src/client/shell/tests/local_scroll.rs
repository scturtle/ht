use super::*;
use crate::client::shell::local_scroll::LocalScrollTarget;

const TARGETS: [LocalScrollTarget; 2] = [LocalScrollTarget::Help, LocalScrollTarget::Navigator];

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn press(state: &mut ClientShellState, rect: Rect, row: u16) -> ClientShellInput {
    mouse(state, MouseEventKind::Down(MouseButton::Left), rect.x, row)
}

fn drag(state: &mut ClientShellState, rect: Rect, row: u16) -> ClientShellInput {
    mouse(state, MouseEventKind::Drag(MouseButton::Left), rect.x, row)
}

fn release(state: &mut ClientShellState, rect: Rect, row: u16) -> ClientShellInput {
    mouse(state, MouseEventKind::Up(MouseButton::Left), rect.x, row)
}

/// Every local scrollbar event stays inside the client: no pane input and no
/// Core request may leave the shell.
fn assert_input_local(outcome: &ClientShellInput) {
    assert!(outcome.requests.is_empty(), "local scroll reached a pane");
    assert!(outcome.actions.is_empty(), "local scroll queued an action");
}

fn hit_map(
    target: LocalScrollTarget,
    track: Rect,
    metrics: crate::pane::ScrollMetrics,
) -> ShellHitMap {
    let mut hits = ShellHitMap::default();
    match target {
        LocalScrollTarget::Help => {
            hits.help_scrollbar = track;
            hits.help_scroll_metrics = Some(metrics);
        }
        LocalScrollTarget::Navigator => {
            hits.navigator_scrollbar = track;
            hits.navigator_scroll_metrics = Some(metrics);
        }
    }
    hits
}

fn metrics(target: LocalScrollTarget, state: &ClientShellState) -> crate::pane::ScrollMetrics {
    match target {
        LocalScrollTarget::Help => state.hits.help_scroll_metrics,
        LocalScrollTarget::Navigator => state.hits.navigator_scroll_metrics,
    }
    .expect("scroll metrics")
}

fn track(target: LocalScrollTarget, state: &ClientShellState) -> Rect {
    match target {
        LocalScrollTarget::Help => state.hits.help_scrollbar,
        LocalScrollTarget::Navigator => state.hits.navigator_scrollbar,
    }
}

fn drag_target(state: &ClientShellState) -> Option<(LocalScrollTarget, u16)> {
    match state.chrome_drag {
        Some(ClientChromeDrag::LocalScrollbar {
            target,
            grab_row_offset,
        }) => Some((target, grab_row_offset)),
        _ => None,
    }
}

/// The first visible row the viewport currently holds.
fn local_start(target: LocalScrollTarget, state: &ClientShellState) -> usize {
    match target {
        LocalScrollTarget::Help => match state.overlay.as_ref() {
            Some(ClientShellOverlay::Help(help)) => help.scroll,
            _ => panic!("help overlay"),
        },
        LocalScrollTarget::Navigator => match state.overlay.as_ref() {
            Some(ClientShellOverlay::Navigator(navigator)) => navigator.scroll,
            _ => panic!("navigator overlay"),
        },
    }
}

/// A composed shell whose `target` scrollbar overflows its viewport.
struct Fixture {
    state: ClientShellState,
    size: (u16, u16),
    target: LocalScrollTarget,
}

impl Fixture {
    fn new(target: LocalScrollTarget) -> Self {
        let (snapshot, size) = match target {
            LocalScrollTarget::Help => (snapshot(), (106, 30)),
            LocalScrollTarget::Navigator => (many_panes(), (106, 24)),
        };
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.mobile_width_threshold = 0;
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(snapshot));
        state.set_pane_surface(surface());
        match target {
            LocalScrollTarget::Help => {
                state.overlay = Some(ClientShellOverlay::Help(ClientHelpOverlay {
                    query: TextEditor::default(),
                    search_focused: false,
                    scroll: 0,
                }));
            }
            LocalScrollTarget::Navigator => state.open_navigator_overlay(),
        }
        let mut fixture = Self {
            state,
            size,
            target,
        };
        fixture.compose();
        assert!(
            !fixture.track().is_empty()
                && metrics(target, &fixture.state).max_offset_from_bottom > 0,
            "{target:?} needs an overflowing scrollbar"
        );
        assert!(
            metrics(target, &fixture.state).offset_from_bottom
                == metrics(target, &fixture.state).max_offset_from_bottom,
            "{target:?} starts at the top"
        );
        fixture
    }

    fn compose(&mut self) {
        self.state.compose(self.size.0, self.size.1).expect("frame");
    }

    fn track(&self) -> Rect {
        track(self.target, &self.state)
    }

    fn offset(&self) -> usize {
        metrics(self.target, &self.state).offset_from_bottom
    }

    fn max_offset(&self) -> usize {
        metrics(self.target, &self.state).max_offset_from_bottom
    }
}

fn many_panes() -> ClientShellSnapshot {
    let mut projected = snapshot();
    for index in 2..=60 {
        let mut pane = projected.panes[0].clone();
        pane.pane_id = format!("pane_{index}");
        pane.label = Some(format!("agent {index}"));
        pane.focused = false;
        projected.panes.push(pane);
    }
    projected
}

#[test]
fn local_scrollbar_press_and_drag_keep_the_grab_offset_and_clamp_rows() {
    // Synthetic tracks of arbitrary size and offset: the shared math is target
    // independent, so two observable viewports cover the arbitrary geometry and
    // both targets are exercised end to end in the shared-event test below.
    for target in TARGETS {
        let track = Rect::new(3, 5, 1, 8);
        for (max, viewport) in [(10, 4), (2, 1)] {
            let at_top = crate::pane::ScrollMetrics {
                offset_from_bottom: max,
                max_offset_from_bottom: max,
                viewport_rows: viewport,
            };
            let thumb = crate::ui::scrollbar_thumb(at_top, track).expect("thumb");
            let mut state = Fixture::new(target).state;
            state.hits = hit_map(target, track, at_top);

            // The thumb under the pointer is grabbed, never re-centred.
            let mut outcome = ClientShellInput::default();
            state.press_local_scrollbar(target, track.y, &mut outcome);
            assert_eq!(
                drag_target(&state),
                Some((target, 0)),
                "{target:?} must keep the thumb under the pointer at the top"
            );
            assert_eq!(local_start(target, &state), 0, "{target:?} jumped");
            assert!(!outcome.repaint, "{target:?} repainted on a grab");

            // Dragging keeps the grab offset instead of centring on the pointer.
            let grab = thumb.len - 1;
            state.drag_local_scrollbar(target, track.y + grab, grab, &mut outcome);
            assert_eq!(local_start(target, &state), 0, "{target:?} moved");
            state.drag_local_scrollbar(target, track.bottom() + 9, grab, &mut outcome);
            assert_eq!(
                local_start(target, &state),
                max,
                "{target:?} clamped a drag below the track to the last row"
            );
            state.drag_local_scrollbar(target, track.y, 0, &mut outcome);
            assert_eq!(
                local_start(target, &state),
                0,
                "{target:?} a zero grab offset drags the thumb itself"
            );
            assert_input_local(&outcome);
            assert!(outcome.repaint);

            // Track presses seek to the row they name and clamp out-of-bounds rows.
            // Only a moved viewport repaints here; Help is the exception below.
            let seek = |row: u16| {
                let mut state = Fixture::new(target).state;
                state.hits = hit_map(target, track, at_top);
                let mut outcome = ClientShellInput::default();
                state.press_local_scrollbar(target, row, &mut outcome);
                assert!(
                    drag_target(&state).is_none(),
                    "{target:?} grabbed a thumb it did not press"
                );
                assert_input_local(&outcome);
                let start = local_start(target, &state);
                assert!(
                    outcome.repaint,
                    "Help seeks and Navigator selection request a repaint"
                );
                start
            };
            assert_eq!(seek(track.bottom() - 1), max, "{target:?} last row");
            assert_eq!(seek(track.y - 3), 0, "{target:?} row above the track");
            assert_eq!(
                seek(track.bottom() + 7),
                max,
                "{target:?} row below the track"
            );
        }
    }
}

fn zero() -> crate::pane::ScrollMetrics {
    crate::pane::ScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        viewport_rows: 0,
    }
}

#[test]
fn local_scrollbar_without_a_usable_track_is_ignored() {
    let scrollable = crate::pane::ScrollMetrics {
        offset_from_bottom: 3,
        max_offset_from_bottom: 3,
        viewport_rows: 2,
    };
    for target in TARGETS {
        // No paint yet, a painted track with no overflow, and metrics without a track.
        for (case, hits) in [
            ("no track before the first paint", ShellHitMap::default()),
            (
                "painted track with no overflow",
                hit_map(target, Rect::new(2, 2, 1, 6), zero()),
            ),
            (
                "metrics without a track",
                hit_map(target, Rect::default(), scrollable),
            ),
        ] {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.hits = hits;
            let mut outcome = ClientShellInput::default();
            state.press_local_scrollbar(target, 3, &mut outcome);
            state.drag_local_scrollbar(target, 9, 1, &mut outcome);
            assert!(
                drag_target(&state).is_none(),
                "{target:?} started a drag with {case}"
            );
            assert!(!outcome.repaint, "{target:?} repainted with {case}");
            assert_input_local(&outcome);
            assert!(state.overlay.is_none(), "{target:?} with {case}");
        }
    }
}

#[test]
fn every_local_scrollbar_shares_the_same_press_track_drag_and_release() {
    for target in TARGETS {
        let mut fixture = Fixture::new(target);
        let track = fixture.track();
        let max = fixture.max_offset();
        let thumb =
            crate::ui::scrollbar_thumb(metrics(target, &fixture.state), track).expect("thumb");
        let grab = thumb.len - 1;

        // Pressing the thumb grabs it without jumping the viewport.
        let outcome = press(&mut fixture.state, track, thumb.top + grab);
        assert_input_local(&outcome);
        assert_eq!(drag_target(&fixture.state), Some((target, grab)));
        fixture.compose();
        assert_eq!(fixture.offset(), max, "{target:?} jumped on thumb press");

        // A drag that keeps the pointer on the thumb never moves the viewport.
        let outcome = drag(&mut fixture.state, track, thumb.top + grab);
        assert_input_local(&outcome);
        fixture.compose();
        assert_eq!(fixture.offset(), max, "{target:?} moved without dragging");

        // Dragging by the grab offset keeps the gesture anchored at the top, so
        // the last row of the track reaches the end of the list.
        let outcome = drag(&mut fixture.state, track, track.bottom() + 5);
        assert_input_local(&outcome);
        fixture.compose();
        assert_eq!(fixture.offset(), 0, "{target:?} did not reach the end");
        assert_eq!(drag_target(&fixture.state), Some((target, grab)));

        // Releasing ends the gesture and never reaches a pane.
        let outcome = release(&mut fixture.state, track, track.bottom() + 5);
        assert_input_local(&outcome);
        assert!(fixture.state.chrome_drag.is_none());
        fixture.compose();
        assert_eq!(fixture.offset(), 0);

        // A track press seeks to the end the pointer names, here back to the top.
        let outcome = press(&mut fixture.state, track, track.y);
        assert_input_local(&outcome);
        assert!(
            drag_target(&fixture.state).is_none(),
            "{target:?} grabbed a thumb it did not press"
        );
        fixture.compose();
        assert_eq!(fixture.offset(), max, "{target:?} track press did not seek");
    }
}

#[test]
fn navigator_selection_follows_the_dragged_viewport_and_help_only_scrolls() {
    let mut navigator = Fixture::new(LocalScrollTarget::Navigator);
    let max = navigator.max_offset();
    let selected_before = ClientNavigatorTarget::Pane {
        pane_id: "pane_1".into(),
    };
    if let Some(ClientShellOverlay::Navigator(overlay)) = navigator.state.overlay.as_mut() {
        // Selected above the viewport the track press drags to: painting reveals
        // the selection unless the seek also moves it into the new viewport.
        overlay.selected = Some(selected_before.clone());
        overlay.scroll = 0;
    } else {
        panic!("navigator overlay");
    }
    navigator.compose();
    let track = navigator.track();
    let outcome = press(&mut navigator.state, track, track.bottom() - 1);
    assert_input_local(&outcome);
    assert!(
        drag_target(&navigator.state).is_none(),
        "a track press seeks instead of grabbing"
    );
    navigator.compose();
    assert_eq!(navigator.offset(), 0, "navigator did not seek to the end");
    let Some(ClientShellOverlay::Navigator(overlay)) = navigator.state.overlay.as_ref() else {
        panic!("navigator overlay");
    };
    assert_eq!(overlay.scroll, max, "navigator kept the stale viewport row");
    let selected = overlay
        .selected
        .clone()
        .expect("selection follows the drag");
    assert_ne!(
        selected, selected_before,
        "selection must move into the dragged viewport"
    );
    assert!(
        navigator
            .state
            .hits
            .navigator_rows
            .iter()
            .any(|(_, target)| *target == selected),
        "selection must stay inside the dragged viewport"
    );

    let mut help = Fixture::new(LocalScrollTarget::Help);
    let track = help.track();
    let outcome = press(&mut help.state, track, track.bottom() - 1);
    assert_input_local(&outcome);
    help.compose();
    assert_eq!(help.offset(), 0, "help did not seek to the end");
    assert_eq!(
        help.state.hits.help_max_scroll,
        match help.state.overlay.as_ref() {
            Some(ClientShellOverlay::Help(overlay)) => overlay.scroll,
            _ => panic!("help overlay"),
        },
        "help scrolls its own wrapped rows"
    );
}

#[test]
fn closing_the_overlay_during_a_drag_never_scrolls_another_viewport() {
    for target in [LocalScrollTarget::Help, LocalScrollTarget::Navigator] {
        let mut fixture = Fixture::new(target);
        let track = fixture.track();
        let thumb =
            crate::ui::scrollbar_thumb(metrics(target, &fixture.state), track).expect("thumb");
        let outcome = press(&mut fixture.state, track, thumb.top);
        assert_input_local(&outcome);
        assert_eq!(drag_target(&fixture.state), Some((target, 0)));

        fixture.state.overlay = None;
        let outcome = drag(&mut fixture.state, track, track.bottom() + 5);
        assert_input_local(&outcome);
        assert!(fixture.state.overlay.is_none());

        let outcome = release(&mut fixture.state, track, track.bottom() + 5);
        assert_input_local(&outcome);
        assert!(fixture.state.chrome_drag.is_none());
    }
}

#[test]
fn help_track_press_repaints_even_when_the_seek_target_is_already_current() {
    let mut fixture = Fixture::new(LocalScrollTarget::Help);
    let track = fixture.track();
    let painted = metrics(LocalScrollTarget::Help, &fixture.state);
    let row = track.bottom() - 1;
    // The painted metrics can be one frame old: Help may already hold exactly the
    // offset this press seeks to, which is why the press has to repaint anyway.
    let sought = painted
        .max_offset_from_bottom
        .saturating_sub(crate::ui::scrollbar_offset_from_row(painted, track, row));
    let current = local_start(LocalScrollTarget::Help, &fixture.state);
    assert_ne!(sought, current, "the fixture must start somewhere else");
    let Some(ClientShellOverlay::Help(help)) = fixture.state.overlay.as_mut() else {
        panic!("help overlay");
    };
    help.scroll = sought;

    let outcome = press(&mut fixture.state, track, row);
    assert_input_local(&outcome);
    assert!(
        outcome.repaint,
        "a Help track press repaints even when its offset already matches"
    );
    assert_eq!(local_start(LocalScrollTarget::Help, &fixture.state), sought);
    assert!(
        drag_target(&fixture.state).is_none(),
        "a track press must not grab"
    );
}
