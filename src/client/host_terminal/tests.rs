//! Host-terminal regression tests, grouped by implementation concern.

mod geometry {
    use std::io;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use super::super::geometry::{
        cell_size_fallback, current_terminal_geometry_with, pack_cell_size, resize_poll_loop_with,
        HostGeometry, HostPixels,
    };
    use crate::client::events::ClientLoopEvent;
    use crate::input::mouse::Position;

    #[test]
    fn resize_poll_loop_reports_signals_and_geometry_changes() {
        let initial = (120, 40, 8, 16, true);
        for (signalled, new_size, expected_report) in [
            (true, initial, true),
            (false, initial, false),
            (false, (121, 40, 8, 16, true), true),
            (false, (120, 41, 8, 16, true), true),
            (false, (120, 40, 9, 18, true), true),
            (false, (120, 40, 8, 16, false), true),
        ] {
            let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel(8);
            let should_quit = AtomicBool::new(false);
            resize_poll_loop_with(
                resize_tx,
                initial,
                |last_cell_size| {
                    assert_eq!(last_cell_size, (8, 16));
                    should_quit.store(true, Ordering::Release);
                    Ok(new_size)
                },
                || signalled,
                &should_quit,
            );

            match resize_rx.try_recv() {
                Ok(ClientLoopEvent::Resize(cols, rows, width, height, exact)) => {
                    assert!(expected_report);
                    assert_eq!((cols, rows, width, height, exact), new_size);
                }
                Err(_) => assert!(!expected_report),
                Ok(_) => panic!("expected a resize event"),
            }
            assert!(resize_rx.try_recv().is_err());
        }
    }

    #[test]
    fn unavailable_terminal_grid_is_not_fabricated() {
        let err = current_terminal_geometry_with(0, None, None, || {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "terminal is gone",
            ))
        })
        .expect_err("an unavailable terminal must not produce fallback geometry");

        assert_eq!(err.kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn resize_poll_loop_keeps_polling_through_a_transient_zero_grid() {
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel(8);
        let should_quit = Arc::new(AtomicBool::new(false));
        let quit = Arc::clone(&should_quit);
        let mut samples = vec![
            Err(crate::platform::zero_sized_terminal_grid_error()),
            Ok((80u16, 24u16, 8u32, 16u32, true)),
        ]
        .into_iter();
        let mut signals = vec![true, false].into_iter();

        resize_poll_loop_with(
            resize_tx,
            (80, 24, 8, 16, true),
            move |_| match samples.next() {
                Some(sample) => sample,
                None => {
                    quit.store(true, Ordering::Release);
                    Ok((80, 24, 8, 16, true))
                }
            },
            move || signals.next().unwrap_or(false),
            &should_quit,
        );

        // The transient zero-sized sample must keep the loop polling and retain the
        // pending resize signal, so the recovered size re-reports exactly once even
        // though it matches the last known size, and no failure is reported.
        let mut events = Vec::new();
        while let Ok(event) = resize_rx.try_recv() {
            events.push(event);
        }
        assert!(
            matches!(
                events.as_slice(),
                [ClientLoopEvent::Resize(80, 24, 8, 16, true)]
            ),
            "expected exactly one recovered resize report"
        );
    }

    #[test]
    fn resize_poll_loop_detaches_once_on_a_real_terminal_failure() {
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel(8);
        let should_quit = Arc::new(AtomicBool::new(false));
        let mut samples = vec![
            Err(crate::platform::zero_sized_terminal_grid_error()),
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "terminal is gone",
            )),
        ]
        .into_iter();

        resize_poll_loop_with(
            resize_tx,
            (80, 24, 8, 16, true),
            move |_| {
                samples
                    .next()
                    .expect("polling must stop after a real terminal failure")
            },
            || false,
            &should_quit,
        );

        // The transient sample must not stop the loop, but the following real
        // failure must report exactly one terminal-unavailable event and terminate.
        let mut events = Vec::new();
        while let Ok(event) = resize_rx.try_recv() {
            events.push(event);
        }
        assert!(
            matches!(
                events.as_slice(),
                [ClientLoopEvent::TerminalUnavailable(err)]
                    if err.kind() == io::ErrorKind::NotConnected
            ),
            "expected exactly one terminal-unavailable event"
        );
    }

    #[test]
    fn exact_terminal_geometry_takes_precedence_over_fallbacks() {
        let geometry = current_terminal_geometry_with(
            pack_cell_size(10, 20),
            Some((9, 18)),
            Some((147, 41, 13, 27)),
            || panic!("an exact snapshot must not read the terminal grid again"),
        )
        .expect("exact geometry is already complete");

        assert_eq!(geometry, (147, 41, 13, 27, true));
    }

    #[test]
    fn missing_pixel_geometry_uses_reported_or_default_cell_size() {
        for (reported, expected) in [(pack_cell_size(13, 27), (13, 27)), (0, (8, 16))] {
            let geometry = current_terminal_geometry_with(reported, None, None, || Ok((80, 24)))
                .expect("a valid grid must retain fallback pixel geometry");

            assert_eq!(geometry, (80, 24, expected.0, expected.1, false));
        }
    }

    #[test]
    fn missing_pixel_geometry_keeps_a_valid_terminal_grid() {
        let geometry = current_terminal_geometry_with(0, Some((9, 18)), None, || Ok((80, 24)))
            .expect("grid geometry remains valid without pixel dimensions");

        assert_eq!(geometry, (80, 24, 9, 18, false));
    }

    #[test]
    fn cell_size_fallback_prefers_reported_then_previous_size() {
        assert_eq!(cell_size_fallback(0, None), (8, 16));
        assert_eq!(cell_size_fallback(0, Some((11, 22))), (11, 22));
        assert_eq!(
            cell_size_fallback(pack_cell_size(10, 21), Some((11, 22))),
            (10, 21)
        );
        assert_eq!(cell_size_fallback(pack_cell_size(10, 0), None), (8, 16));
        assert_eq!(cell_size_fallback(pack_cell_size(0, 21), None), (8, 16));
    }

    #[test]
    fn integer_cell_pitch_ignores_trailing_pixel_remainder() {
        let geometry = HostGeometry::new(127, 31, 1_276, 626).unwrap();
        assert_eq!(geometry.cell(220, 1), Some((21, 0)));
        assert_eq!(geometry.cell(221, 61), Some((22, 3)));
        let right_pane = ratatui::layout::Rect::new(22, 3, 105, 20);
        assert_eq!(
            HostPixels {
                x: 221,
                y: 61,
                geometry,
            }
            .pane_position(right_pane, 1_050, 400),
            Some(Position::Pixels { x: 1, y: 1 })
        );
        assert_eq!(geometry.cell(1_270, 620), Some((126, 30)));
        assert_eq!(
            HostPixels {
                x: 1_270,
                y: 460,
                geometry,
            }
            .pane_position(right_pane, 1_050, 400),
            Some(Position::Pixels { x: 1_050, y: 400 })
        );

        let full_grid = ratatui::layout::Rect::new(0, 0, 127, 31);
        for x in 1_271..=1_276 {
            assert_eq!(geometry.cell(x, 1), Some((126, 0)));
            assert_eq!(
                HostPixels { x, y: 1, geometry }.pane_position(full_grid, 1_270, 620),
                None
            );
        }
        for y in 621..=626 {
            assert_eq!(geometry.cell(1, y), Some((0, 30)));
            assert_eq!(
                HostPixels { x: 1, y, geometry }.pane_position(full_grid, 1_270, 620),
                None
            );
        }
        assert_eq!(geometry.cell(1_277, 1), None);
        assert_eq!(geometry.cell(1, 627), None);
    }

    #[test]
    fn geometry_rejects_outside_pixels_and_maps_cells() {
        let geometry = HostGeometry::new(80, 24, 800, 480).unwrap();
        assert_eq!(geometry.cell(1, 1), Some((0, 0)));
        assert_eq!(geometry.cell(800, 480), Some((79, 23)));
        assert_eq!(geometry.cell(801, 1), None);
        assert_eq!(geometry.cell(0, 1), None);
    }
}

mod modes {
    use crossterm::event::KeyboardEnhancementFlags;

    use super::super::modes::{
        host_modify_other_keys_mode_for_env, ime_compatible_keyboard_enhancement_flags,
        set_host_kitty_keyboard_report_all, ModifyOtherKeysMode,
    };

    #[test]
    fn host_keyboard_report_all_replaces_the_current_herdr_stack_entry() {
        let mut output = Vec::new();

        set_host_kitty_keyboard_report_all(&mut output, true).unwrap();
        set_host_kitty_keyboard_report_all(&mut output, false).unwrap();

        assert_eq!(output, b"\x1b[<1u\x1b[>31u\x1b[<1u\x1b[>7u");
    }

    #[test]
    fn keyboard_enhancement_flags_stay_ime_compatible() {
        let flags = ime_compatible_keyboard_enhancement_flags();

        assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
        assert!(!flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
    }

    #[test]
    fn modify_other_keys_policy_uses_tmux_then_host_markers() {
        use ModifyOtherKeysMode::{Mode1, Mode2};

        for (tmux, term_program, wezterm, alacritty, expected) in [
            (false, None, false, false, None),
            (false, Some("unknown"), false, false, None),
            (false, Some("WezTerm"), false, false, Some(Mode1)),
            (false, Some("wezterm"), false, false, Some(Mode1)),
            (false, None, true, false, Some(Mode1)),
            (false, None, false, true, Some(Mode1)),
            (true, None, false, false, Some(Mode2)),
            (true, Some("WezTerm"), false, false, Some(Mode2)),
            (true, None, true, true, Some(Mode2)),
        ] {
            assert_eq!(
                host_modify_other_keys_mode_for_env(tmux, term_program, wezterm, alacritty),
                expected,
                "tmux={tmux}, term_program={term_program:?}, wezterm={wezterm}, alacritty={alacritty}",
            );
        }
    }

    #[test]
    fn modify_other_keys_mode_sequences_are_stable() {
        assert_eq!(ModifyOtherKeysMode::Mode1.set_sequence(), b"\x1b[>4;1m");
        assert_eq!(ModifyOtherKeysMode::Mode2.set_sequence(), b"\x1b[>4;2m");
    }
}

mod probe {
    use super::super::probe::{
        consume_host_keyboard_probe_responses, host_escape_disambiguation_confirmed,
        host_terminal_probe, HostKeyboardProbeResponses,
    };

    #[test]
    fn host_capability_probe_consumes_fragmented_responses_and_preserves_input() {
        let stream = b"before\x1b[?1016;2$y-middle-\x1b[?7u-\x1b[?1;2cafter";

        for split in 1..stream.len() {
            let mut buffered = Vec::new();
            let mut responses = HostKeyboardProbeResponses::default();
            buffered.extend_from_slice(&stream[..split]);
            consume_host_keyboard_probe_responses(&mut buffered, &mut responses);
            buffered.extend_from_slice(&stream[split..]);
            consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

            assert_eq!(responses.sgr_pixel_mouse_status, Some(2), "split {split}");
            assert_eq!(responses.flags, Some(7), "split {split}");
            assert!(responses.primary_device_attributes, "split {split}");
            assert_eq!(buffered, b"before-middle--after", "split {split}");
        }
    }

    #[test]
    fn host_capability_probe_accepts_only_usable_sgr_pixel_mouse_states() {
        for status in 0..=4 {
            let mut buffered = format!("\x1b[?1016;{status}$y").into_bytes();
            let mut responses = HostKeyboardProbeResponses::default();

            consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

            assert_eq!(
                host_terminal_probe(&responses).sgr_pixel_mouse,
                Some(status == 2),
                "status {status}"
            );
            assert!(buffered.is_empty());
        }
    }

    #[test]
    fn host_probe_keeps_a_pixel_mouse_answer_without_device_attributes() {
        let mut buffered = b"\x1b[?1016;0$y".to_vec();
        let mut responses = HostKeyboardProbeResponses::default();
        consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

        assert!(!responses.primary_device_attributes);
        assert_eq!(host_terminal_probe(&responses).sgr_pixel_mouse, Some(false));
    }

    #[test]
    fn host_keyboard_probe_preserves_typed_input_before_responses() {
        let mut buffered = b"aPtyped\x1b[?1016;2$y\x1b[?7u\x1b[?1;2c".to_vec();
        let mut responses = HostKeyboardProbeResponses::default();

        consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

        assert!(host_escape_disambiguation_confirmed(&responses));
        assert_eq!(host_terminal_probe(&responses).sgr_pixel_mouse, Some(true));
        assert_eq!(buffered, b"aPtyped");
    }

    #[test]
    fn host_keyboard_probe_requires_disambiguation_bit_and_device_attributes() {
        for (flags, expected) in [(0, false), (2, false), (7, true)] {
            let mut buffered = format!("\x1b[?{flags}u\x1b[?1;2c").into_bytes();
            let mut responses = HostKeyboardProbeResponses::default();

            consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

            assert_eq!(host_escape_disambiguation_confirmed(&responses), expected);
            assert!(buffered.is_empty());
        }
    }

    #[test]
    fn host_keyboard_probe_requires_flags_before_device_attributes() {
        let mut buffered = b"\x1b[?1;2c\x1b[?7uinput".to_vec();
        let mut responses = HostKeyboardProbeResponses::default();

        consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

        assert_eq!(responses.flags, None);
        assert!(responses.primary_device_attributes);
        assert_eq!(buffered, b"input");
    }

    #[test]
    fn host_keyboard_probe_preserves_response_shaped_payloads() {
        let opaque =
            b"\x1b[200~paste \x1b[?1016;2$y \x1b[?1u \x1b[?1;2c\x1b[201~-\x1bPdata \x1b[?7u\x1b\\";
        let mut buffered = [opaque.as_slice(), b"\x1b[?1016;2$y\x1b[?7u\x1b[?1;2c"].concat();
        let mut responses = HostKeyboardProbeResponses::default();

        consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

        assert!(host_escape_disambiguation_confirmed(&responses));
        assert_eq!(host_terminal_probe(&responses).sgr_pixel_mouse, Some(true));
        assert_eq!(buffered, opaque);
    }

    #[test]
    fn host_keyboard_probe_preserves_malformed_responses() {
        let mut buffered = b"a\x1b[?7;1ub\x1b[?65536uc\x1b[?1016;5$y\x1b[?1006;2$y".to_vec();
        let mut responses = HostKeyboardProbeResponses::default();

        consume_host_keyboard_probe_responses(&mut buffered, &mut responses);

        assert_eq!(responses.flags, None);
        assert_eq!(responses.sgr_pixel_mouse_status, None);
        assert!(!responses.primary_device_attributes);
        assert_eq!(
            buffered,
            b"a\x1b[?7;1ub\x1b[?65536uc\x1b[?1016;5$y\x1b[?1006;2$y"
        );
    }
}

mod setup {
    use super::super::modes::set_host_kitty_keyboard_report_all;
    use super::super::setup::{
        enable_host_color_scheme_reports, refresh_test_guard, write_terminal_restore_postlude,
        TerminalGuard,
    };

    #[test]
    fn host_color_scheme_reports_enable_sequence() {
        let mut output = Vec::new();
        enable_host_color_scheme_reports(&mut output).unwrap();
        assert_eq!(output, b"\x1b[?2031h");
    }

    #[test]
    fn terminal_restore_postlude_resets_only_claimed_color_reports() {
        for reset_color_reports in [false, true] {
            let mut output = Vec::new();
            write_terminal_restore_postlude(&mut output, reset_color_reports).unwrap();
            let expected: &[u8] = if reset_color_reports {
                b"\x1b[?2031l\x1b[?25h\x1b[0 q"
            } else {
                b"\x1b[?25h\x1b[0 q"
            };
            assert_eq!(output, expected);
        }
    }

    fn refresh_bytes(guard: &TerminalGuard, report_all_keys: bool, focus: bool) -> Vec<u8> {
        let mut output = Vec::new();
        guard
            .refresh_host_keyboard_protocol(&mut output, report_all_keys, focus)
            .unwrap();
        output
    }

    #[test]
    fn host_keyboard_protocol_refresh_resends_the_setup_modes() {
        // Focus-in on a kitty host: replace Herdr's stack entry, keep paste on.
        let kitty = refresh_test_guard(true, true, None);
        assert_eq!(
            refresh_bytes(&kitty, false, false),
            b"\x1b[<1u\x1b[>7u\x1b[?2004h"
        );
        // Resize while a pane wants all keys, on a modifyOtherKeys host too.
        let both = refresh_test_guard(true, true, Some(b"\x1b[>4;1m"));
        assert_eq!(
            refresh_bytes(&both, true, true),
            b"\x1b[<1u\x1b[>31u\x1b[>4;1m\x1b[?2004h\x1b[?1004h"
        );
        // A host that never confirmed the kitty protocol gets no kitty flags.
        let legacy = refresh_test_guard(false, true, None);
        assert_eq!(
            refresh_bytes(&legacy, false, true),
            b"\x1b[?2004h\x1b[?1004h"
        );
        // Direct attach leaves keyboard modes to the attached pane.
        let direct = refresh_test_guard(true, false, None);
        assert!(refresh_bytes(&direct, false, true).is_empty());
        // Nothing is re-enabled once the terminal is being restored.
        kitty.mark_restore_claimed_for_test();
        assert!(refresh_bytes(&kitty, false, true).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn host_keyboard_protocol_refresh_restores_shift_enter_after_host_reset() {
        use crate::input::{KeyboardProtocol, TerminalKey};
        use crossterm::event::{KeyCode, KeyModifiers};

        let guard = refresh_test_guard(true, true, None);
        let shift_enter = TerminalKey::new(KeyCode::Enter, KeyModifiers::SHIFT);
        // libghostty stands in for the host terminal. `reset` models a host
        // that lost its modes, like a reloaded VS Code window (#4985).
        for reset in [false, true] {
            let host = crate::pane::testing::Screen::new(80, 24);
            // Herdr's setup push.
            host.test_process_pty_bytes(b"\x1b[>7u");
            if reset {
                host.test_process_pty_bytes(b"\x1bc");
                assert_eq!(host.keyboard_protocol(), KeyboardProtocol::Legacy);
                assert_eq!(host.encode_terminal_key(shift_enter.clone()), b"\r");
            }
            for _ in 0..3 {
                host.test_process_pty_bytes(&refresh_bytes(&guard, false, true));
            }
            assert_eq!(
                host.keyboard_protocol(),
                KeyboardProtocol::Kitty { flags: 7 },
                "reset={reset}"
            );
            assert_eq!(
                host.encode_terminal_key(shift_enter.clone()),
                b"\x1b[13;2u",
                "reset={reset}"
            );
            // Refreshes never grow the stack: Herdr's one exit pop restores
            // the host's own keyboard mode.
            host.test_process_pty_bytes(b"\x1b[<1u");
            assert_eq!(
                host.keyboard_protocol(),
                KeyboardProtocol::Legacy,
                "reset={reset}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn host_keyboard_protocol_refresh_keeps_the_outer_mode_across_report_all_toggles() {
        use crate::input::KeyboardProtocol;

        let guard = refresh_test_guard(true, true, None);
        let host = crate::pane::testing::Screen::new(80, 24);
        // Something outside Herdr (a shell or wrapper) already set its own mode.
        host.test_process_pty_bytes(b"\x1b[>1u");
        // Herdr's setup push.
        host.test_process_pty_bytes(b"\x1b[>7u");
        for round in 0..6 {
            let report_all = round % 2 == 0;
            let mut toggle = Vec::new();
            set_host_kitty_keyboard_report_all(&mut toggle, report_all).unwrap();
            host.test_process_pty_bytes(&toggle);
            for focus in [false, true, false] {
                host.test_process_pty_bytes(&refresh_bytes(&guard, report_all, focus));
            }
            let flags = if report_all { 31 } else { 7 };
            assert_eq!(
                host.keyboard_protocol(),
                KeyboardProtocol::Kitty { flags },
                "round {round}"
            );
        }
        // Herdr's one exit pop restores exactly the outer mode.
        host.test_process_pty_bytes(b"\x1b[<1u");
        assert_eq!(
            host.keyboard_protocol(),
            KeyboardProtocol::Kitty { flags: 1 }
        );
    }
}

mod restore {
    use std::cell::Cell;
    use std::io;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    };

    use super::super::modes::clear_host_mouse_reporting;
    use super::super::probe::HostTerminalProbe;
    use super::super::setup::{
        restore_terminal_state_with, setup_terminal_with, terminal_guard_for_test, TerminalGuard,
    };

    const MODIFY_OTHER_KEYS: &[u8] = b"\x1b[>4;2m";
    const KEYBOARD_POP: &[u8] = b"\x1b[<1u";
    const POSTLUDE: &[u8] = b"\x1b[?2031l\x1b[?25h\x1b[0 q";

    #[derive(Default)]
    struct FaultWriter {
        bytes: Vec<u8>,
        attempts: Vec<Vec<u8>>,
        flushes: usize,
        fail_after_bytes: Option<usize>,
        fail_write_call: Option<usize>,
        fail_flush: Option<usize>,
    }

    impl io::Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.attempts.push(bytes.to_vec());
            if self.fail_write_call == Some(self.attempts.len())
                || self.fail_after_bytes == Some(self.bytes.len())
            {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected write failure",
                ));
            }
            let count = self.fail_after_bytes.map_or(bytes.len(), |limit| {
                bytes.len().min(limit.saturating_sub(self.bytes.len()))
            });
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.fail_flush == Some(self.flushes) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected flush failure",
                ));
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct RestoreRecord {
        modes: Vec<(bool, bool, bool)>,
        bytes: Vec<u8>,
        raw_restore_calls: usize,
    }

    fn recorded_guard(raw_restore_fails: bool) -> (TerminalGuard, Arc<Mutex<RestoreRecord>>) {
        let record = Arc::new(Mutex::new(RestoreRecord::default()));
        let callback_record = Arc::clone(&record);
        let guard = terminal_guard_for_test(move |keyboard, modify_other_keys, color_reports| {
            let mut record = callback_record.lock().unwrap();
            record
                .modes
                .push((keyboard, modify_other_keys, color_reports));
            let RestoreRecord {
                bytes,
                raw_restore_calls,
                ..
            } = &mut *record;
            restore_terminal_state_with(
                bytes,
                keyboard,
                modify_other_keys,
                color_reports,
                |_| Ok(()),
                || {
                    *raw_restore_calls += 1;
                    if raw_restore_fails {
                        Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            "injected raw restore failure",
                        ))
                    } else {
                        Ok(())
                    }
                },
            )
        });
        (guard, record)
    }

    fn finish_setup(
        guard: TerminalGuard,
        writer: &mut impl io::Write,
    ) -> io::Result<TerminalGuard> {
        setup_terminal_with(
            guard,
            writer,
            || {
                (
                    HostTerminalProbe {
                        escape_disambiguation: true,
                        sgr_pixel_mouse: Some(false),
                    },
                    b"typed input".to_vec(),
                )
            },
            || Some(MODIFY_OTHER_KEYS),
        )
    }

    fn initialized_guard(raw_restore_fails: bool) -> (TerminalGuard, Arc<Mutex<RestoreRecord>>) {
        let (guard, record) = recorded_guard(raw_restore_fails);
        (finish_setup(guard, &mut Vec::new()).unwrap(), record)
    }

    fn occurrences(bytes: &[u8], sequence: &[u8]) -> usize {
        bytes
            .windows(sequence.len())
            .filter(|part| *part == sequence)
            .count()
    }

    fn assert_single_restore(record: &Mutex<RestoreRecord>, expected: (bool, bool, bool)) {
        let record = record.lock().unwrap();
        assert_eq!(record.modes, [expected]);
        assert_eq!(record.raw_restore_calls, 1);
        assert_eq!(
            occurrences(&record.bytes, KEYBOARD_POP),
            usize::from(expected.0)
        );
        assert_eq!(
            occurrences(&record.bytes, b"\x1b[>4;0m"),
            usize::from(expected.1)
        );
        assert_eq!(
            occurrences(&record.bytes, b"\x1b[?2031l"),
            usize::from(expected.2)
        );
        assert!(record.bytes.ends_with(b"\x1b[?25h\x1b[0 q"));
    }

    #[test]
    fn setup_write_failure_at_every_byte_restores_only_attempted_modes() {
        let (guard, _) = recorded_guard(false);
        let mut complete = FaultWriter::default();
        drop(finish_setup(guard, &mut complete).unwrap());
        let mut initial_mouse_reset = Vec::new();
        clear_host_mouse_reporting(&mut initial_mouse_reset).unwrap();
        let color_start = complete
            .bytes
            .windows(b"\x1b[?2031h".len())
            .position(|bytes| bytes == b"\x1b[?2031h")
            .unwrap();
        let modify_start = complete
            .bytes
            .windows(MODIFY_OTHER_KEYS.len())
            .position(|bytes| bytes == MODIFY_OTHER_KEYS)
            .unwrap();
        for failure_offset in 0..complete.bytes.len() {
            let (guard, record) = recorded_guard(false);
            let mut writer = FaultWriter {
                fail_after_bytes: Some(failure_offset),
                ..FaultWriter::default()
            };
            let Err(error) = finish_setup(guard, &mut writer) else {
                panic!("setup unexpectedly succeeded at byte {failure_offset}");
            };
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(writer.bytes, complete.bytes[..failure_offset]);
            assert_single_restore(
                &record,
                (
                    failure_offset >= initial_mouse_reset.len(),
                    failure_offset >= modify_start,
                    failure_offset >= color_start,
                ),
            );
        }
    }

    #[test]
    fn setup_flush_failures_drop_the_guard_with_recorded_progress() {
        let expected = [
            (false, false, false), // Initial mouse reset, before the keyboard push.
            (true, false, false),  // Keyboard push.
            (true, false, false),  // Mouse reset before enabling capture.
            (true, false, false),  // Mouse capture.
            (true, false, false),  // Paste/focus reporting.
            (true, false, true),   // Color reporting.
            (true, true, true),    // modifyOtherKeys.
            (true, true, true),    // Line wrapping.
        ];
        let (guard, _) = recorded_guard(false);
        let mut complete = FaultWriter::default();
        drop(finish_setup(guard, &mut complete).unwrap());
        assert_eq!(complete.flushes, expected.len());
        for (index, expected) in expected.into_iter().enumerate() {
            let (guard, record) = recorded_guard(false);
            let mut writer = FaultWriter {
                fail_flush: Some(index + 1),
                ..FaultWriter::default()
            };
            let Err(error) = finish_setup(guard, &mut writer) else {
                panic!("setup unexpectedly succeeded at flush {}", index + 1);
            };
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_single_restore(&record, expected);
        }
    }

    #[test]
    fn setup_error_is_preserved_when_drop_restore_also_fails() {
        let (guard, record) = recorded_guard(true);
        let mut writer = FaultWriter {
            fail_after_bytes: Some(0),
            ..FaultWriter::default()
        };
        let Err(error) = finish_setup(guard, &mut writer) else {
            panic!("setup unexpectedly succeeded");
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_single_restore(&record, (false, false, false));
    }

    #[test]
    fn setup_query_panics_drop_the_guard_with_current_progress() {
        for panic_in_probe in [true, false] {
            let (guard, record) = recorded_guard(false);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                setup_terminal_with(
                    guard,
                    &mut Vec::new(),
                    || {
                        assert!(!panic_in_probe, "injected capability query panic");
                        (HostTerminalProbe::default(), Vec::new())
                    },
                    || panic!("injected modifyOtherKeys policy panic"),
                )
            }));
            assert!(result.is_err());
            assert_single_restore(&record, (true, false, !panic_in_probe));
        }
    }

    #[test]
    fn setup_preserves_probe_results_and_buffered_input() {
        let (mut guard, record) = initialized_guard(false);
        assert!(guard.host_escape_disambiguation_active());
        assert_eq!(guard.host_sgr_pixel_mouse(), Some(false));
        assert_eq!(guard.take_buffered_host_input(), b"typed input");
        assert!(guard.take_buffered_host_input().is_empty());
        guard.restore().unwrap();
        assert_single_restore(&record, (true, true, true));
    }

    #[test]
    fn explicit_restore_success_or_failure_is_not_retried_by_drop_or_panic_callback() {
        for fails in [false, true] {
            let (guard, record) = initialized_guard(fails);
            let panic_restore = guard.panic_restore();
            let result = guard.restore(); // Consumes the guard and runs its Drop too.
            if fails {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotConnected);
            } else {
                result.unwrap();
            }
            panic_restore();
            panic_restore();
            assert_single_restore(&record, (true, true, true));
        }
    }

    #[test]
    fn failed_restore_write_is_not_retried_by_drop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let guard = terminal_guard_for_test(move |keyboard, modify, colors| {
            callback_calls.fetch_add(1, Ordering::Relaxed);
            let mut writer = FaultWriter {
                fail_after_bytes: Some(0),
                ..FaultWriter::default()
            };
            restore_terminal_state_with(
                &mut writer,
                keyboard,
                modify,
                colors,
                |_| Ok(()),
                || Ok(()),
            )
        });
        let guard = finish_setup(guard, &mut Vec::new()).unwrap();
        let panic_restore = guard.panic_restore();
        assert_eq!(
            guard.restore().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        panic_restore();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn panic_restore_claim_prevents_refresh_explicit_restore_and_drop_from_repeating_io() {
        for fails in [false, true] {
            let (guard, record) = initialized_guard(fails);
            let panic_restore = guard.panic_restore();
            panic_restore();
            panic_restore();
            let mut refreshed = Vec::new();
            guard
                .refresh_host_keyboard_protocol(&mut refreshed, true, true)
                .unwrap();
            assert!(refreshed.is_empty());
            guard.restore().unwrap(); // Already claimed, regardless of the first attempt's result.
            panic_restore();
            assert_single_restore(&record, (true, true, true));
        }
    }

    #[test]
    fn drop_claim_prevents_late_panic_callback_from_repeating_io() {
        for fails in [false, true] {
            let (guard, record) = initialized_guard(fails);
            let panic_restore = guard.panic_restore();
            drop(guard);
            panic_restore();
            assert_single_restore(&record, (true, true, true));
        }
    }

    #[test]
    fn concurrent_panic_callbacks_and_drop_restore_once() {
        for _ in 0..8 {
            let (guard, record) = initialized_guard(true);
            let panic_restore = Arc::new(guard.panic_restore());
            let barrier = Arc::new(Barrier::new(9));
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    let panic_restore = Arc::clone(&panic_restore);
                    std::thread::spawn(move || {
                        barrier.wait();
                        panic_restore();
                    })
                })
                .collect();
            barrier.wait();
            drop(guard);
            for worker in workers {
                worker.join().unwrap();
            }
            assert_single_restore(&record, (true, true, true));
        }
    }

    #[test]
    fn panic_during_explicit_restore_does_not_retry_on_unwind() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let guard = terminal_guard_for_test(move |_, _, _| {
            callback_calls.fetch_add(1, Ordering::Relaxed);
            panic!("injected restore panic");
        });
        let panic_restore = guard.panic_restore();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| guard.restore())).is_err()
        );
        panic_restore();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn restore_modes_are_conditional_and_preserve_the_outer_keyboard_stack() {
        for mask in 0..8 {
            let (keyboard, modify, colors) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
            let mut bytes = Vec::new();
            restore_terminal_state_with(
                &mut bytes,
                keyboard,
                modify,
                colors,
                |_| Ok(()),
                || Ok(()),
            )
            .unwrap();
            assert_eq!(occurrences(&bytes, KEYBOARD_POP), usize::from(keyboard));
            assert_eq!(occurrences(&bytes, b"\x1b[>4;0m"), usize::from(modify));
            assert_eq!(occurrences(&bytes, b"\x1b[?2031l"), usize::from(colors));
            let host = crate::pane::testing::Screen::new(80, 24);
            host.test_process_pty_bytes(b"\x1b[>1u"); // Outer shell/wrapper.
            if keyboard {
                host.test_process_pty_bytes(b"\x1b[>7u"); // Herdr's one stack entry.
            }
            host.test_process_pty_bytes(&bytes);
            assert_eq!(
                host.keyboard_protocol(),
                crate::input::KeyboardProtocol::Kitty { flags: 1 }
            );
        }
    }

    #[test]
    fn early_cleanup_failures_do_not_skip_raw_restore_or_postlude() {
        let mut complete = FaultWriter::default();
        restore_terminal_state_with(&mut complete, true, true, true, |_| Ok(()), || Ok(()))
            .unwrap();
        // The last two writes belong to the color/cursor postlude, whose errors
        // are propagated rather than treated as best-effort earlier cleanup.
        for failed_write in 1..=complete.attempts.len() - 2 {
            let mut writer = FaultWriter {
                fail_write_call: Some(failed_write),
                ..FaultWriter::default()
            };
            let raw_calls = Cell::new(0);
            restore_terminal_state_with(
                &mut writer,
                true,
                true,
                true,
                |_| Err(io::Error::other("injected graphics cleanup failure")),
                || {
                    raw_calls.set(raw_calls.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(raw_calls.get(), 1);
            assert!(
                writer.bytes.ends_with(POSTLUDE),
                "failed write {failed_write}"
            );
        }
    }

    #[test]
    fn raw_restore_and_postlude_errors_preserve_error_priority() {
        for raw_fails in [false, true] {
            for writer_fails in [false, true] {
                let mut writer = FaultWriter {
                    fail_after_bytes: writer_fails.then_some(0),
                    ..FaultWriter::default()
                };
                let raw_calls = Cell::new(0);
                let result = restore_terminal_state_with(
                    &mut writer,
                    true,
                    true,
                    true,
                    |_| Ok(()),
                    || {
                        raw_calls.set(raw_calls.get() + 1);
                        if raw_fails {
                            Err(io::Error::new(
                                io::ErrorKind::NotConnected,
                                "raw restore failure",
                            ))
                        } else {
                            Ok(())
                        }
                    },
                );
                assert_eq!(raw_calls.get(), 1);
                if raw_fails {
                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotConnected);
                } else if writer_fails {
                    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
                } else {
                    result.unwrap();
                }
                if writer_fails {
                    assert_eq!(writer.attempts.last().unwrap(), b"\x1b[?2031l");
                } else {
                    assert!(writer.bytes.ends_with(POSTLUDE));
                }
            }
        }
    }

    #[test]
    fn postlude_flush_error_is_propagated_after_raw_restore() {
        let mut complete = FaultWriter::default();
        restore_terminal_state_with(&mut complete, true, true, true, |_| Ok(()), || Ok(()))
            .unwrap();
        let mut writer = FaultWriter {
            fail_flush: Some(complete.flushes),
            ..FaultWriter::default()
        };
        let raw_calls = Cell::new(0);
        let result = restore_terminal_state_with(
            &mut writer,
            true,
            true,
            true,
            |_| Ok(()),
            || {
                raw_calls.set(raw_calls.get() + 1);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(raw_calls.get(), 1);
        assert!(writer.bytes.ends_with(POSTLUDE));
    }
}

mod theme {
    use std::io;

    use super::super::theme::{write_host_query, write_host_theme_query};
    use crate::terminal::HOST_COLOR_SCHEME_QUERY_SEQUENCE;

    #[derive(Default)]
    struct QueryWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl io::Write for QueryWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn theme_query_without_palette_writes_only_default_colors_and_flushes_once() {
        let mut writer = QueryWriter::default();
        write_host_theme_query(&mut writer, false).unwrap();
        assert_eq!(writer.bytes, b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        assert_eq!(writer.flushes, 1);
    }

    #[test]
    fn theme_query_with_palette_keeps_all_indices_in_order_and_flushes_once() {
        let mut writer = QueryWriter::default();
        write_host_theme_query(&mut writer, true).unwrap();
        let text = std::str::from_utf8(&writer.bytes).unwrap();
        let mut queries = text.split_terminator("\x1b\\");
        assert_eq!(queries.next(), Some("\x1b]10;?"));
        assert_eq!(queries.next(), Some("\x1b]11;?"));
        for index in 0..=u8::MAX {
            assert_eq!(queries.next(), Some(format!("\x1b]4;{index};?").as_str()));
        }
        assert_eq!(queries.next(), None);
        assert_eq!(writer.flushes, 1);
    }

    #[test]
    fn appearance_query_writes_only_the_appearance_query_and_flushes_once() {
        let mut writer = QueryWriter::default();
        write_host_query(&mut writer, HOST_COLOR_SCHEME_QUERY_SEQUENCE.as_bytes()).unwrap();
        assert_eq!(writer.bytes, b"\x1b[?996n");
        assert_eq!(writer.flushes, 1);
    }
}
