//! Raw input unit tests and test-only framing helpers.

use super::*;
use crossterm::event::{KeyCode, KeyEventKind};

/// Parse raw terminal input bytes into a list of `RawInputEvent`s.
///
/// This directly extracts events without going through a channel, making it
/// suitable for synchronous use.
pub fn parse_raw_input_bytes_sync(data: &[u8]) -> Vec<RawInputEvent> {
    let mut framer = RawInputFramer::default();
    let mut events = framer.push(data);
    events.extend(framer.flush_timeout());
    events
}

/// Returns the UTF-8 payload when `data` is exactly one complete bracketed paste.
pub(crate) fn complete_text_bracketed_paste(data: &[u8]) -> Option<&str> {
    if !data.starts_with(BRACKETED_PASTE_START) {
        return None;
    }
    let end = find_subsequence(data, BRACKETED_PASTE_END)?;
    if end + BRACKETED_PASTE_END.len() != data.len() {
        return None;
    }
    std::str::from_utf8(&data[BRACKETED_PASTE_START.len()..end]).ok()
}

/// Byte-framing conformance oracle; Core input uses typed paste events.
pub(crate) fn is_complete_text_bracketed_paste(data: &[u8]) -> bool {
    complete_text_bracketed_paste(data).is_some()
}

/// Test-only byte framing and event decoding.
#[derive(Default)]
pub(crate) struct RawInputFramer {
    byte_framer: RawInputByteFramer,
}

impl RawInputFramer {
    pub(crate) fn for_host_input() -> Self {
        Self {
            byte_framer: RawInputByteFramer::for_host_input(),
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<RawInputEvent> {
        events_from_framed_chunks(self.byte_framer.push(data))
    }

    pub(crate) fn flush_timeout(&mut self) -> Vec<RawInputEvent> {
        events_from_framed_chunks(self.byte_framer.flush_timeout())
    }
}

pub(crate) fn events_require_host_surface_redraw(
    events: &[RawInputEvent],
    redraw_on_focus_gained: bool,
) -> bool {
    redraw_on_focus_gained
        && events
            .iter()
            .any(|event| matches!(event, RawInputEvent::OuterFocusGained))
}

pub(crate) fn events_require_host_terminal_appearance_query(events: &[RawInputEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RawInputEvent::OuterFocusGained))
}

pub(crate) fn events_require_host_terminal_theme_query(events: &[RawInputEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RawInputEvent::HostColorSchemeChanged(_)))
}

impl RawInputByteFramer {
    fn age_partial_host_color_reply(&mut self, by: Duration) {
        self.partial_host_color_reply_read_at = self
            .partial_host_color_reply_read_at
            .and_then(|at| at.checked_sub(by));
    }

    fn age_host_reply_activity(&mut self, by: Duration) {
        self.host_reply_activity_at = self
            .host_reply_activity_at
            .and_then(|at| at.checked_sub(by));
    }
}

fn assert_raw_key(event: RawInputEvent, code: KeyCode, modifiers: KeyModifiers) {
    let RawInputEvent::Key(key) = event else {
        panic!("expected key");
    };
    assert_eq!(key.code, code);
    assert_eq!(key.modifiers, modifiers);
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    assert_eq!(hex.len() % 2, 0, "hex string must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|idx| u8::from_str_radix(&hex[idx..idx + 2], 16).unwrap())
        .collect()
}

fn parse_fixture_key_code(value: &str) -> KeyCode {
    match value {
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "esc" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "insert" => KeyCode::Insert,
        "delete" => KeyCode::Delete,
        value if value.starts_with("char:") => {
            KeyCode::Char(value.trim_start_matches("char:").chars().next().unwrap())
        }
        other => panic!("unsupported fixture key code: {other}"),
    }
}

fn parse_fixture_modifiers(value: &str) -> KeyModifiers {
    if value == "-" || value.is_empty() {
        return KeyModifiers::empty();
    }

    let mut modifiers = KeyModifiers::empty();
    for part in value.split('+') {
        match part {
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "alt" => modifiers |= KeyModifiers::ALT,
            "control" => modifiers |= KeyModifiers::CONTROL,
            "super" => modifiers |= KeyModifiers::SUPER,
            "hyper" => modifiers |= KeyModifiers::HYPER,
            "meta" => modifiers |= KeyModifiers::META,
            other => panic!("unsupported fixture modifier: {other}"),
        }
    }
    modifiers
}

#[test]
fn parses_kitty_shift_letter_release() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[108:76;2:3u").unwrap()
    else {
        panic!("expected key");
    };
    assert_eq!(consumed, 13);
    assert_eq!(key.code, KeyCode::Char('l'));
    assert_eq!(key.modifiers, KeyModifiers::SHIFT);
    assert_eq!(key.kind, KeyEventKind::Release);
    assert_eq!(key.shifted_codepoint, Some('L' as u32));
}

#[test]
fn parses_bracketed_paste() {
    let (RawInputEvent::Paste(text), consumed) =
        extract_one_event(b"\x1b[200~hello\x1b[201~rest").unwrap()
    else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello");
    assert_eq!(consumed, 17);
}

#[test]
fn complete_text_bracketed_paste_requires_one_exact_utf8_sequence() {
    assert_eq!(
        complete_text_bracketed_paste(b"\x1b[200~hello\x1b[201~"),
        Some("hello")
    );
    assert!(!is_complete_text_bracketed_paste(b"\x1b[200~hello"));
    assert!(!is_complete_text_bracketed_paste(
        b"\x1b[200~hello\x1b[201~rest"
    ));
    assert!(!is_complete_text_bracketed_paste(
        b"\x1b[200~one\x1b[201~\x1b[200~two\x1b[201~"
    ));
    assert!(!is_complete_text_bracketed_paste(b"\x1b[200~\xff\x1b[201~"));
}

#[test]
fn sgr_mouse_report_parser_accepts_only_complete_reports() {
    for (input, expected) in [
        (b"\x1b[<35;321;241M".as_slice(), Some((321, 241))),
        (b"\x1b[<0;1;2m".as_slice(), Some((1, 2))),
        (b"key".as_slice(), None),
        (b"\x1b[<0;1;2Mkey".as_slice(), None),
        (b"\x1b[<0;1M".as_slice(), None),
        (b"\x1b[<0;1;2;3M".as_slice(), None),
    ] {
        assert_eq!(
            parse_sgr_mouse_report(input).map(|report| (report.x, report.y)),
            expected,
            "{input:?}"
        );
    }
}

#[test]
fn parses_sgr_mouse() {
    let (RawInputEvent::Mouse(mouse), consumed) = extract_one_event(b"\x1b[<0;20;10M").unwrap()
    else {
        panic!("expected mouse");
    };
    assert_eq!(consumed, 11);
    assert_eq!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
    assert_eq!(mouse.column, 19);
    assert_eq!(mouse.row, 9);
    assert_eq!(mouse.modifiers, KeyModifiers::empty());
}

#[test]
fn parses_default_mouse_encoding() {
    let mut framer = RawInputFramer::default();
    let events = framer.push(b"\x1b[MCN1");
    let [RawInputEvent::Mouse(mouse)] = events.as_slice() else {
        panic!("expected one mouse event");
    };
    assert_eq!(mouse.kind, MouseEventKind::Moved);
    assert_eq!((mouse.column, mouse.row), (45, 16));
    assert_eq!(mouse.modifiers, KeyModifiers::empty());
}

#[test]
fn rejected_default_mouse_frame_preserves_trailing_input() {
    let mut framer = RawInputFramer::default();
    let events = framer.push(b"\x1b[M\x82AAx");

    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], RawInputEvent::Unsupported));
    assert_raw_key(
        events.into_iter().nth(1).unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn parses_extended_button_drag_as_mouse_motion() {
    for input in [
        b"\x1b[<160;20;10M".as_slice(),
        b"\x1b[<161;20;10M".as_slice(),
    ] {
        let (RawInputEvent::Mouse(mouse), _) = extract_one_event(input).unwrap() else {
            panic!("expected mouse");
        };
        assert_eq!(mouse.kind, MouseEventKind::Moved);
        assert_eq!((mouse.column, mouse.row), (19, 9));
    }
}

#[test]
fn parses_sgr_mouse_observable_modifiers() {
    let cases = [
        (b"\x1b[<8;20;10M".as_slice(), KeyModifiers::ALT),
        (b"\x1b[<16;20;10M".as_slice(), KeyModifiers::CONTROL),
        (
            b"\x1b[<24;20;10M".as_slice(),
            KeyModifiers::ALT | KeyModifiers::CONTROL,
        ),
    ];

    for (input, expected) in cases {
        let (RawInputEvent::Mouse(mouse), _) = extract_one_event(input).unwrap() else {
            panic!("expected mouse");
        };
        assert_eq!(mouse.modifiers, expected);
        assert!(!mouse.modifiers.contains(KeyModifiers::SUPER));
    }
}

#[test]
fn parses_host_default_color_response_with_st() {
    let (RawInputEvent::HostDefaultColor { kind, color }, consumed) =
        extract_one_event(b"\x1b]10;rgb:cccc/dddd/eeee\x1b\\").unwrap()
    else {
        panic!("expected host color response");
    };
    assert_eq!(consumed, 25);
    assert_eq!(kind, DefaultColorKind::Foreground);
    assert_eq!(
        color,
        RgbColor {
            r: 0xcc,
            g: 0xdd,
            b: 0xee
        }
    );
}

#[test]
fn parses_host_default_color_response_with_bel() {
    let (RawInputEvent::HostDefaultColor { kind, color }, consumed) =
        extract_one_event(b"\x1b]11;#112233\x07").unwrap()
    else {
        panic!("expected host color response");
    };
    assert_eq!(consumed, 13);
    assert_eq!(kind, DefaultColorKind::Background);
    assert_eq!(
        color,
        RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33
        }
    );
}

#[test]
fn parses_host_palette_color_response() {
    let (RawInputEvent::HostPaletteColors { colors }, consumed) =
        extract_one_event(b"\x1b]4;7;rgb:1111/2222/3333\x1b\\").unwrap()
    else {
        panic!("expected host palette response");
    };
    assert_eq!(consumed, 26);
    assert_eq!(
        colors,
        vec![(
            7,
            RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }
        )]
    );
}

#[test]
fn parses_legacy_up_arrow() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[A").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 3);
    assert_eq!(key.code, KeyCode::Up);
}

#[test]
fn parses_outer_focus_events() {
    let (event, consumed) = extract_one_event(b"\x1b[I").unwrap();
    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::OuterFocusGained));

    let (event, consumed) = extract_one_event(b"\x1b[O").unwrap();
    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::OuterFocusLost));
}

#[test]
fn outer_focus_gained_requests_host_surface_redraw() {
    let events = parse_raw_input_bytes_sync(b"\x1b[I");
    assert!(events_require_host_surface_redraw(&events, true));
    assert!(!events_require_host_surface_redraw(&events, false));

    let events = parse_raw_input_bytes_sync(b"\x1b[O");
    assert!(!events_require_host_surface_redraw(&events, true));
}

#[test]
fn outer_focus_gained_requests_host_mode_refresh() {
    assert!(events_require_host_mode_refresh(
        &parse_raw_input_bytes_sync(b"\x1b[I")
    ));
    assert!(!events_require_host_mode_refresh(
        &parse_raw_input_bytes_sync(b"\x1b[O")
    ));
}

#[test]
fn outer_focus_gained_requests_host_appearance_query() {
    let gained = parse_raw_input_bytes_sync(b"\x1b[I");
    let lost = parse_raw_input_bytes_sync(b"\x1b[O");
    let scheme_report = parse_raw_input_bytes_sync(b"\x1b[?997;1n");

    assert!(events_require_host_terminal_appearance_query(&gained));
    assert!(!events_require_host_terminal_appearance_query(&lost));
    assert!(!events_require_host_terminal_appearance_query(
        &scheme_report
    ));
    assert!(events_require_host_terminal_theme_query(&scheme_report));
}

#[test]
fn parses_ghostty_color_scheme_reports() {
    for bytes in [
        GHOSTTY_COLOR_SCHEME_DARK_REPORT,
        GHOSTTY_COLOR_SCHEME_LIGHT_REPORT,
    ] {
        let events = parse_raw_input_bytes_sync(bytes);
        assert_eq!(events.len(), 1, "bytes: {bytes:?}");
        assert!(matches!(
            events[0],
            RawInputEvent::HostColorSchemeChanged(HostAppearance::Dark | HostAppearance::Light)
        ));
        assert!(events_require_host_terminal_theme_query(&events));
    }
}

#[test]
fn ghostty_color_scheme_report_parser_is_exact() {
    for bytes in [
        b"\x1b[?997;0n".as_slice(),
        b"\x1b[?997;3n".as_slice(),
        b"\x1b[?998;1n".as_slice(),
    ] {
        let events = parse_raw_input_bytes_sync(bytes);
        assert_eq!(events.len(), 1, "bytes: {bytes:?}");
        assert!(matches!(events[0], RawInputEvent::Unsupported));
        assert!(!events_require_host_terminal_theme_query(&events));
    }
}

#[test]
fn raw_input_framer_reassembles_split_color_scheme_report() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[?997;").is_empty());
    let events = framer.push(b"1n");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostColorSchemeChanged(HostAppearance::Dark)
    ));
}

#[test]
fn parses_host_cell_size_report() {
    let events = parse_raw_input_bytes_sync(b"\x1b[6;21;10t");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostCellSizeReport {
            width_px: 10,
            height_px: 21,
        }
    ));
}

#[test]
fn host_cell_size_report_parser_is_exact() {
    for bytes in [
        // Zero dimensions carry no usable cell size.
        b"\x1b[6;0;10t".as_slice(),
        b"\x1b[6;21;0t".as_slice(),
        // Missing or extra parameters.
        b"\x1b[6;21t".as_slice(),
        b"\x1b[6;21;10;3t".as_slice(),
        // Other XTWINOPS reports must not be mistaken for a cell size.
        b"\x1b[4;1610;777t".as_slice(),
        b"\x1b[8;37;161t".as_slice(),
        // Non-numeric parameters.
        b"\x1b[6;21;1-t".as_slice(),
    ] {
        assert!(
            parse_host_cell_size_report(bytes).is_none(),
            "bytes: {bytes:?}"
        );
    }
}

#[test]
fn split_color_scheme_timeout_does_not_swallow_legacy_alt_bracket() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b[".to_vec()]);
}

#[test]
fn raw_input_byte_framer_discards_timed_out_split_color_scheme_report_tail() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1n").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn parses_xterm_alt_up_arrow() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[1;3A").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 6);
    assert_eq!(key.code, KeyCode::Up);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_legacy_alt_backspace() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b\x7f").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 2);
    assert_eq!(key.code, KeyCode::Backspace);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_kitty_alt_backspace() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[127;3u").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::Backspace);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_enhanced_pageup_press() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[5;1:1~").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::PageUp);
    assert_eq!(key.modifiers, KeyModifiers::empty());
    assert_eq!(key.kind, KeyEventKind::Press);
}

#[test]
fn parses_enhanced_pagedown_release() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[6;1:3~").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::PageDown);
    assert_eq!(key.modifiers, KeyModifiers::empty());
    assert_eq!(key.kind, KeyEventKind::Release);
}

#[test]
fn raw_input_family_matrix_is_covered() {
    let cases: &[(&[u8], KeyCode, KeyModifiers)] = &[
        (b"\x02", KeyCode::Char('b'), KeyModifiers::CONTROL),
        (b"\r", KeyCode::Enter, KeyModifiers::empty()),
        (b"\t", KeyCode::Tab, KeyModifiers::empty()),
        (b"\x7f", KeyCode::Backspace, KeyModifiers::empty()),
        (b"\x1b[A", KeyCode::Up, KeyModifiers::empty()),
        (b"\x1b[1;3A", KeyCode::Up, KeyModifiers::ALT),
        (b"\x1b\x7f", KeyCode::Backspace, KeyModifiers::ALT),
        (b"\x1b[127;3u", KeyCode::Backspace, KeyModifiers::ALT),
        (b"\x1b[57420;1u", KeyCode::Down, KeyModifiers::empty()),
        (b"\x1b[57423;1u", KeyCode::Home, KeyModifiers::empty()),
        (b"\x1bOq", KeyCode::Char('1'), KeyModifiers::empty()),
        (b"\x1b[14~", KeyCode::F(4), KeyModifiers::empty()),
        (b"\x1b[11;2~", KeyCode::F(1), KeyModifiers::SHIFT),
        (b"\x1b[13;1:1~", KeyCode::F(3), KeyModifiers::empty()),
        (b"\x1b[14;3~", KeyCode::F(4), KeyModifiers::ALT),
        (b"\x1b[57364;1u", KeyCode::F(1), KeyModifiers::empty()),
        (b"\x1b[57366;1u", KeyCode::F(3), KeyModifiers::empty()),
        (b"\x1b[57366;2u", KeyCode::F(3), KeyModifiers::SHIFT),
        (b"\x1b[57375;1u", KeyCode::F(12), KeyModifiers::empty()),
        (b"\x1b[57376;1u", KeyCode::F(13), KeyModifiers::empty()),
        (b"\x1b[49:33;2:1u", KeyCode::Char('1'), KeyModifiers::SHIFT),
    ];

    for (bytes, code, modifiers) in cases {
        let (event, consumed) = extract_one_event(bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_raw_key(event, *code, *modifiers);
    }
}

#[test]
fn raw_framer_waits_for_application_keypad_sequence_final_byte() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1bO").is_empty());
    let events = framer.push(b"q");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::empty(),
    );
}

#[test]
fn unsupported_ss3_sequence_stays_unsupported() {
    let (event, consumed) = extract_one_event(b"\x1bOz").unwrap();

    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::Unsupported));
}

#[test]
fn parses_modified_rxvt_f_key_alias() {
    let (event, consumed) = extract_one_event(b"\x1b[14;3~").unwrap();

    assert_eq!(consumed, 7);
    assert_raw_key(event, KeyCode::F(4), KeyModifiers::ALT);
}

#[test]
fn flushes_lone_escape_after_timeout() {
    let mut framer = RawInputFramer::default();
    assert!(framer.push(&[ESC]).is_empty());

    let events = framer.flush_timeout();
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Esc,
        KeyModifiers::empty(),
    );
}

#[test]
fn parses_raw_ctrl_b() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x02").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 1);
    assert_eq!(key.code, KeyCode::Char('b'));
    assert_eq!(key.modifiers, KeyModifiers::CONTROL);
}

#[test]
fn parses_raw_lf_as_ctrl_j() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\n").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 1);
    assert_eq!(key.code, KeyCode::Char('j'));
    assert_eq!(key.modifiers, KeyModifiers::CONTROL);
}

fn assert_fixture_extracts_whole_events(corpus: &str, macos_layout: bool) {
    for line in corpus.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns: Vec<_> = line.split('\t').collect();
        if columns.len() == 5 {
            columns.push("");
        }

        if macos_layout {
            if columns.len() == 6 {
                columns.push("");
            }
            assert_eq!(
                columns.len(),
                7,
                "macOS fixture row must have 7 columns: {line}"
            );
            if columns[2].is_empty() {
                continue;
            }
            let bytes = decode_hex(columns[2]);
            let (event, consumed) = extract_one_event(&bytes).unwrap();
            assert_eq!(
                consumed,
                bytes.len(),
                "fixture should extract a whole event: {line}"
            );
            assert_raw_key(
                event,
                parse_fixture_key_code(columns[3]),
                parse_fixture_modifiers(columns[4]),
            );
        } else {
            if columns.len() == 5 {
                columns.push("");
            }
            let (bytes_hex, code, modifiers) = match columns.len() {
                6 => {
                    if columns[1].chars().all(|ch| ch.is_ascii_hexdigit()) {
                        (columns[1], columns[2], columns[3])
                    } else {
                        (columns[2], columns[3], columns[4])
                    }
                }
                7 => (columns[2], columns[3], columns[4]),
                _ => panic!("fixture row must have 6 or 7 columns: {line}"),
            };
            assert!(
                bytes_hex.chars().all(|ch| ch.is_ascii_hexdigit()),
                "non-hex fixture bytes: {bytes_hex} in {line}"
            );
            let bytes = decode_hex(bytes_hex);
            let (event, consumed) = extract_one_event(&bytes).unwrap();
            assert_eq!(
                consumed,
                bytes.len(),
                "fixture should extract a whole event: {line}"
            );
            assert_raw_key(
                event,
                parse_fixture_key_code(code),
                parse_fixture_modifiers(modifiers),
            );
        }
    }
}

#[test]
fn raw_input_corpus_fixture_extracts_whole_events() {
    let corpus = include_str!("../tests/fixtures/keyboard_protocol_corpus.tsv");
    assert_fixture_extracts_whole_events(corpus, false);
}

#[test]
fn raw_input_macos_terminal_variants_fixture_extracts_whole_events() {
    let corpus = include_str!("../tests/fixtures/macos_terminal_variants.tsv");
    assert_fixture_extracts_whole_events(corpus, true);
}

#[test]
fn raw_input_linux_terminal_variants_fixture_extracts_whole_events() {
    let corpus = include_str!("../tests/fixtures/linux_terminal_variants.tsv");
    assert_fixture_extracts_whole_events(corpus, false);
}

#[test]
fn chunked_legacy_arrow_waits_for_completion() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[A");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Up,
        KeyModifiers::empty(),
    );
}

#[test]
fn lone_escape_is_buffered_until_timeout_flush() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.flush_timeout();
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Esc,
        KeyModifiers::empty(),
    );
}

#[test]
fn escape_followed_by_arrow_before_flush_does_not_emit_escape() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[B");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Down,
        KeyModifiers::empty(),
    );
}

#[test]
fn escape_followed_by_sgr_mouse_before_flush_does_not_emit_text() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[<65;43;26M");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 42,
            row: 25,
            ..
        })
    ));
}

#[test]
fn lone_escape_then_complete_sgr_mouse_report_emits_both_events() {
    for report in [b"\x1b[<35;10;20M".as_slice(), b"\x1b[<35;10;20m".as_slice()] {
        let mut framer = RawInputFramer::default();

        assert!(framer.push(b"\x1b").is_empty());
        let events = framer.push(report);

        assert_eq!(events.len(), 2);
        let mut events = events.into_iter();
        assert_raw_key(events.next().unwrap(), KeyCode::Esc, KeyModifiers::empty());
        assert!(matches!(
            events.next().unwrap(),
            RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 9,
                row: 19,
                ..
            })
        ));
        assert!(framer.flush_timeout().is_empty());
    }
}

#[test]
fn lone_escape_then_default_mouse_report_emits_both_events() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"\x1b[MCN1");

    assert_eq!(events.len(), 2);
    let mut events = events.into_iter();
    assert_raw_key(events.next().unwrap(), KeyCode::Esc, KeyModifiers::empty());
    assert!(matches!(
        events.next().unwrap(),
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 45,
            row: 16,
            ..
        })
    ));
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn legacy_doubled_escape_alt_arrow_remains_one_event() {
    let mut framer = RawInputFramer::default();

    let events = framer.push(b"\x1b\x1b[A");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Up,
        KeyModifiers::ALT,
    );
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn non_macos_host_input_splits_lone_escape_from_arrow() {
    let mut framer = RawInputByteFramer::for_host_input();

    assert_eq!(
        framer.push(b"\x1b\x1b[D"),
        vec![b"\x1b".to_vec(), b"\x1b[D".to_vec()]
    );
}

#[test]
fn macos_host_input_policy_preserves_legacy_doubled_escape_alt_arrow() {
    let mut framer = RawInputByteFramer::with_host_input_policy(true);

    assert_eq!(framer.push(b"\x1b\x1b[D"), vec![b"\x1b\x1b[D".to_vec()]);
}

/// WezTerm with `enable_kitty_keyboard` sends an Escape press as a bare
/// ESC and its release as a kitty report (#1266). A quick tap lands both in
/// one read, or in two reads before the idle flush.
const WEZTERM_ESCAPE_TAP: (&[u8], &[u8]) = (b"\x1b", b"\x1b[27;1:3u");

fn assert_escape_press_then_release(chunks: Vec<Vec<u8>>, taps: usize) {
    let events = events_from_framed_chunks(chunks);
    assert_eq!(events.len(), taps * 2, "{events:?}");
    for pair in events.chunks(2) {
        assert!(
            matches!(&pair[0], RawInputEvent::Key(key)
                if key.code == KeyCode::Esc
                    && key.modifiers.is_empty()
                    && key.kind == KeyEventKind::Press),
            "{events:?}"
        );
        assert!(
            matches!(&pair[1], RawInputEvent::Key(key)
                if key.code == KeyCode::Esc
                    && key.modifiers.is_empty()
                    && key.kind == KeyEventKind::Release),
            "{events:?}"
        );
    }
}

#[test]
fn preserved_doubled_escape_policy_splits_escape_press_from_kitty_release() {
    let (press, release) = WEZTERM_ESCAPE_TAP;
    let tap = [press, release].concat();

    // One read.
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    let mut chunks = framer.push(&tap);
    chunks.extend(framer.flush_timeout());
    assert_eq!(chunks, vec![press.to_vec(), release.to_vec()]);
    assert_escape_press_then_release(chunks, 1);

    // Two reads before the idle flush.
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    let mut chunks = framer.push(press);
    chunks.extend(framer.push(release));
    chunks.extend(framer.flush_timeout());
    assert_escape_press_then_release(chunks, 1);

    // Two quick taps (double Esc) in one read.
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    let mut chunks = framer.push(&[tap.clone(), tap].concat());
    chunks.extend(framer.flush_timeout());
    assert_escape_press_then_release(chunks, 2);

    // The release split across reads still separates once it completes.
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    let mut chunks = framer.push(b"\x1b\x1b[27;");
    chunks.extend(framer.push(b"1:3u"));
    chunks.extend(framer.flush_timeout());
    assert_escape_press_then_release(chunks, 1);
}

#[test]
fn preserved_doubled_escape_policy_splits_the_tap_at_every_read_boundary() {
    let (press, release) = WEZTERM_ESCAPE_TAP;
    let tap = [press, release].concat();
    for taps in [tap.clone(), [tap.clone(), tap.clone()].concat()] {
        let count = taps.len() / tap.len();
        for split in 1..taps.len() {
            let mut framer = RawInputByteFramer::with_host_input_policy(true);
            let mut chunks = framer.push(&taps[..split]);
            chunks.extend(framer.push(&taps[split..]));
            chunks.extend(framer.flush_timeout());
            assert_escape_press_then_release(chunks, count);
        }
    }
}

#[test]
fn preserved_doubled_escape_policy_splits_escape_before_alt_kitty_report() {
    // Escape, then Alt+a reported by a kitty host: two keys, not Alt+Alt+a.
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    let mut chunks = framer.push(b"\x1b\x1b[97;3u");
    chunks.extend(framer.flush_timeout());
    assert_eq!(chunks, vec![b"\x1b".to_vec(), b"\x1b[97;3u".to_vec()]);
    let events = events_from_framed_chunks(chunks);
    assert!(
        matches!(&events[..], [RawInputEvent::Key(esc), RawInputEvent::Key(alt_a)]
            if esc.code == KeyCode::Esc && esc.modifiers.is_empty()
                && alt_a.code == KeyCode::Char('a') && alt_a.modifiers == KeyModifiers::ALT),
        "{events:?}"
    );
}

#[test]
fn preserved_doubled_escape_policy_keeps_legacy_alt_sequences_whole() {
    for bytes in [
        &b"\x1b\x1b[D"[..],
        b"\x1b\x1b[1;3D",
        b"\x1b\x1b[3~",
        b"\x1b\x1bOA",
    ] {
        let mut framer = RawInputByteFramer::with_host_input_policy(true);
        let mut chunks = framer.push(bytes);
        chunks.extend(framer.flush_timeout());
        assert_eq!(chunks, vec![bytes.to_vec()], "{bytes:?}");
    }
    for bytes in [
        &b"\x1b[u"[..],
        b"\x1b[?7u",
        b"\x1b[;3u",
        b"\x1b[27;1:3",
        b"\x1b[27;1:3~",
        b"\x1b[D",
        b"\x1bOA",
    ] {
        assert!(!starts_with_kitty_key_report(bytes), "{bytes:?}");
    }
    for bytes in [&b"\x1b[27u"[..], b"\x1b[27;1:3u", b"\x1b[55:47;2;47u"] {
        assert!(starts_with_kitty_key_report(bytes), "{bytes:?}");
    }
}

#[test]
fn sgr_mouse_sequence_split_after_button_prefix_is_reassembled_before_timeout() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    let events = framer.push(b"5;58;30M");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 57,
            row: 29,
            ..
        })
    ));
}

#[test]
fn timed_out_split_sgr_mouse_tail_is_discarded_and_following_input_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"5;58;30Mx");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn captured_sgr_mouse_tail_after_second_idle_flush_is_discarded() {
    let mut framer = RawInputByteFramer::for_host_input();

    // Issue #3911, 2026-09-13 07:02:14 UTC: this prefix timed out,
    // then its tail arrived 33 ms later. The Unix reader flushes again
    // after 10 ms of continued idle following the first discard.
    assert!(framer.push(b"\x1b[<3").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"5;28;31M"), Vec::<Vec<u8>>::new());
}

#[test]
fn timed_out_sgr_mouse_invalid_completion_is_preserved_after_idle() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"M"), vec![b"M".to_vec()]);
}

#[test]
fn timed_out_sgr_mouse_completion_survives_read_splits_and_idle() {
    let tail = b"5;28;31M";
    for split in 0..=tail.len() {
        let mut framer = RawInputByteFramer::for_host_input();
        assert!(framer.push(b"\x1b[<3").is_empty());
        assert!(framer.flush_timeout().is_empty());
        assert!(framer.flush_timeout().is_empty());
        assert!(framer.push(&tail[..split]).is_empty());
        assert!(framer.flush_timeout().is_empty());
        assert!(framer.flush_timeout().is_empty());
        let mut rest = tail[split..].to_vec();
        rest.extend_from_slice(b"x\x1b[A");
        assert_eq!(framer.push(&rest), vec![b"x".to_vec(), b"\x1b[A".to_vec()]);
        assert!(framer.timed_out_mouse_prefix.is_none());
        assert!(!framer.has_pending_input());
    }
}

#[test]
fn timed_out_sgr_mouse_invalid_syntax_releases_continuation() {
    for tail in [
        b"5;0;31M".as_slice(), // zero coordinate
        b"5;;31M",             // empty field
        b"5;28;31;1M",         // extra field (the general parser is permissive)
        b"999;28;31M",         // button overflow
        b"5;65536;31M",        // coordinate overflow
    ] {
        let mut framer = RawInputByteFramer::default();
        assert!(framer.push(b"\x1b[<3").is_empty());
        assert!(framer.flush_timeout().is_empty());
        assert_eq!(framer.push(tail).concat(), tail);
        assert!(framer.timed_out_mouse_prefix.is_none());
    }
}

#[test]
fn timed_out_sgr_mouse_interruption_preserves_text_and_new_events() {
    for suffix in [
        b"x".as_slice(),
        b"\x1b[A",                  // new key sequence
        b"\x1b[200~paste\x1b[201~", // bracketed paste
        "\u{4f60}".as_bytes(),      // UTF-8 split across reads
    ] {
        let mut framer = RawInputByteFramer::default();
        assert!(framer.push(b"\x1b[<3").is_empty());
        assert!(framer.flush_timeout().is_empty());
        assert!(framer.push(b"5;28;").is_empty());
        assert!(framer.flush_timeout().is_empty());
        let mut chunks = Vec::new();
        for byte in suffix {
            chunks.extend(framer.push(&[*byte]));
        }
        let mut expected = b"5;28;".to_vec();
        expected.extend_from_slice(suffix);
        assert_eq!(chunks.concat(), expected);
        assert!(framer.timed_out_mouse_prefix.is_none());
        assert_eq!(framer.push(b"123M").concat(), b"123M");
    }
}

#[test]
fn timed_out_sgr_mouse_budget_includes_prefix_and_preserves_overflow() {
    let prefix = b"\x1b[<35;1;";
    let remaining = MAX_DISCARDED_CONTROL_TAIL_BYTES - prefix.len();
    let mut framer = RawInputByteFramer::default();
    assert!(framer.push(prefix).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let mut valid_tail = vec![b'0'; remaining - 2];
    valid_tail.extend_from_slice(b"1M");
    assert!(framer.push(&valid_tail).is_empty()); // complete exactly at limit
    assert!(framer.timed_out_mouse_prefix.is_none());

    for length in [remaining, remaining + 1024] {
        let mut framer = RawInputByteFramer::default();
        assert!(framer.push(prefix).is_empty());
        assert!(framer.flush_timeout().is_empty());
        let tail = vec![b'0'; length];
        assert_eq!(framer.push(&tail).concat(), tail);
        assert!(framer.timed_out_mouse_prefix.is_none());
        assert_eq!(framer.push(b"1Mtext").concat(), b"1Mtext");
    }

    let mut framer = RawInputByteFramer::default();
    assert!(framer.push(prefix).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let mut tail = vec![b'0'; remaining - 1];
    assert!(framer.push(&tail).is_empty());
    assert!(framer.flush_timeout().is_empty());
    tail.extend_from_slice(b"01M");
    assert_eq!(framer.push(b"01M").concat(), tail);
    assert!(framer.timed_out_mouse_prefix.is_none());
}

#[test]
fn sgr_mouse_tail_after_lone_escape_timeout_is_discarded() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let timeout_events = framer.flush_timeout();
    assert_eq!(timeout_events.len(), 1);
    assert_raw_key(
        timeout_events.into_iter().next().unwrap(),
        KeyCode::Esc,
        KeyModifiers::empty(),
    );

    assert!(framer.push(b"[<65;43;26M").is_empty());
}

#[test]
fn input_after_discarded_complete_sgr_mouse_tail_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);
    let events = framer.push(b"[<65;43;26Mx");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn invalid_orphaned_sgr_mouse_tail_after_escape_timeout_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);

    let events = framer.push(b"[<x");

    assert_eq!(events.len(), 3);
    assert_raw_key(
        events.into_iter().last().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn double_split_sgr_mouse_tail_after_lone_escape_timeout_is_discarded() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);

    assert!(framer.push(b"[<65;4").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"3;26Mx");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn escape_followed_by_alt_char_before_flush_becomes_alt_key() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"b");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('b'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_kitty_sequence_waits_for_completion() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[49:33;2:").is_empty());
    let events = framer.push(b"1u");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::SHIFT,
    );
}

#[test]
fn chunked_bracketed_paste_waits_for_terminator() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[200~hello").is_empty());
    let events = framer.push(b"\x1b[201~");
    assert_eq!(events.len(), 1);
    let RawInputEvent::Paste(text) = &events[0] else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello");
}

#[test]
fn incomplete_bracketed_paste_is_not_flushed_on_timeout() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[200~hello\nworld").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"\x1b[201~");
    assert_eq!(events.len(), 1);
    let RawInputEvent::Paste(text) = &events[0] else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello\nworld");
}

#[test]
fn complete_utf8_char_before_incomplete_char_is_drained() {
    let mut framer = RawInputByteFramer::default();
    let mut input = "你".as_bytes().to_vec();
    input.push("好".as_bytes()[0]);

    assert_eq!(framer.push(&input), vec!["你".as_bytes().to_vec()]);
    assert_eq!(framer.push(&[]), Vec::<Vec<u8>>::new());
}

#[test]
fn incomplete_utf8_prefix_is_not_flushed_on_timeout() {
    let mut framer = RawInputByteFramer::default();
    let prefix = &"好".as_bytes()[..1];

    assert!(framer.push(prefix).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(&"好".as_bytes()[1..]),
        vec!["好".as_bytes().to_vec()]
    );
}

#[test]
fn invalid_utf8_lead_byte_is_flushed_instead_of_buffered_forever() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(&[0xC0]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(!framer.has_pending_input());
}

#[test]
fn complete_utf8_char_before_incomplete_char_survives_timeout_and_next_chunk() {
    let mut framer = RawInputByteFramer::default();
    let mut input = "你".as_bytes().to_vec();
    input.push("好".as_bytes()[0]);

    assert_eq!(framer.push(&input), vec!["你".as_bytes().to_vec()]);
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(&"好".as_bytes()[1..]),
        vec!["好".as_bytes().to_vec()]
    );
}

#[test]
fn alt_utf8_char_drains_as_one_event_before_following_input() {
    let events = parse_raw_input_bytes_sync("\x1béx".as_bytes());
    assert_eq!(events.len(), 2);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_alt_utf8_waits_for_continuation_byte_after_escape() {
    let mut framer = RawInputFramer::default();
    let bytes = "\x1bé".as_bytes();

    assert!(framer.push(&bytes[..2]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(&bytes[2..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_utf8_waits_for_continuation_byte() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(&"é".as_bytes()[..1]).is_empty());
    let events = framer.push(&"é".as_bytes()[1..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::empty(),
    );
}

#[test]
fn chunked_cjk_utf8_waits_for_all_continuation_bytes() {
    let mut framer = RawInputFramer::default();
    let bytes = "好".as_bytes();

    assert!(framer.push(&bytes[..1]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(&bytes[1..2]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(&bytes[2..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('好'),
        KeyModifiers::empty(),
    );
}

#[test]
fn chunked_four_byte_utf8_waits_for_all_continuation_bytes() {
    let mut framer = RawInputFramer::default();
    let bytes = "🙂".as_bytes();

    for split in 1..bytes.len() {
        assert!(framer.push(&bytes[split - 1..split]).is_empty());
        assert!(framer.flush_timeout().is_empty());
    }

    let events = framer.push(&bytes[bytes.len() - 1..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('🙂'),
        KeyModifiers::empty(),
    );
}

#[test]
fn long_multilingual_voice_like_burst_drains_without_truncation() {
    let text = "你好，今天我们测试一段比较长的语音输入。こんにちは。안녕하세요.🙂".repeat(128);
    assert!(
        text.len() > 4096,
        "test input should exceed the client read buffer"
    );
    let mut framer = RawInputByteFramer::default();

    let chunks = framer.push(text.as_bytes());
    let rebuilt: Vec<u8> = chunks.into_iter().flatten().collect();

    assert!(!framer.has_pending_input());
    assert_eq!(rebuilt, text.as_bytes());
}

#[test]
fn long_multilingual_burst_survives_one_byte_chunks_and_timeouts() {
    let text = "中文かなカナ한글🙂，。".repeat(64);
    let mut framer = RawInputByteFramer::default();
    let mut rebuilt = Vec::new();

    for byte in text.as_bytes() {
        rebuilt.extend(
            framer
                .push(std::slice::from_ref(byte))
                .into_iter()
                .flatten(),
        );
        if framer.has_pending_input() {
            assert!(framer.flush_timeout().is_empty());
        }
    }

    rebuilt.extend(framer.flush_timeout().into_iter().flatten());
    assert!(!framer.has_pending_input());
    assert_eq!(rebuilt, text.as_bytes());
}

#[test]
fn parses_ghostty_default_background_response() {
    let events = parse_raw_input_bytes_sync(b"\x1b]11;rgb:2828/2a2a/3636\x07");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x28,
                g: 0x2a,
                b: 0x36
            }
        }
    ));
}

#[test]
fn raw_input_framer_reassembles_split_default_background_response() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    let events = framer.push(b"11;#123456\x07");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            }
        }
    ));
}

#[test]
fn raw_input_byte_framer_discards_split_control_string_after_timeout() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"11;#123456\x07").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
}

#[test]
fn raw_input_byte_framer_keeps_discarding_tail_across_timeout() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1;#123456\x07").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
}

#[test]
fn raw_input_byte_framer_releases_discard_on_implausible_tail() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"a").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"b"), vec![b"b".to_vec()]);
}

#[test]
fn preserved_doubled_escape_is_alt_escape_after_a_short_wait() {
    // macOS keeps legacy ESC ESC whole: Alt+Escape (and Ctrl+Alt+3).
    let mut framer = RawInputByteFramer::with_host_input_policy(true);
    assert!(framer.push(b"\x1b\x1b").is_empty());
    assert_eq!(
        framer.idle_flush_timeout_ms(false),
        RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS
    );
    let chunks = framer.flush_timeout();
    assert_eq!(chunks, vec![b"\x1b\x1b".to_vec()]);
    let events = events_from_framed_chunks(chunks);
    assert!(
        matches!(&events[..], [RawInputEvent::Key(key)]
            if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::ALT),
        "{events:?}"
    );
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
    // A longer Alt sequence still joins up within the window.
    assert!(framer.push(b"\x1b\x1b").is_empty());
    assert_eq!(framer.push(b"[A"), vec![b"\x1b\x1b[A".to_vec()]);
}

#[test]
fn csi_and_ss3_introducers_without_expected_reply_are_alt_keys() {
    for (bytes, code) in [
        (&b"\x1b["[..], KeyCode::Char('[')),
        (b"\x1bO", KeyCode::Char('O')),
    ] {
        let mut framer = RawInputFramer::default();
        assert!(framer.push(bytes).is_empty());
        let events = framer.flush_timeout();
        assert!(
            matches!(&events[..], [RawInputEvent::Key(key)]
                if key.code == code && key.modifiers.contains(KeyModifiers::ALT)),
            "{bytes:?}: {events:?}"
        );
        // The next keystroke is not swallowed as a sequence tail.
        assert_eq!(framer.push(b"a").len(), 1, "{bytes:?}");
    }
}

#[test]
fn parse_raw_input_bytes_sync_does_not_parse_incomplete_strings_as_alt_keys() {
    for bytes in [
        b"\x1b]".as_slice(),
        b"\x1bP".as_slice(),
        b"\x1b_".as_slice(),
        b"\x1b^".as_slice(),
        b"\x1bX".as_slice(),
    ] {
        let events = parse_raw_input_bytes_sync(bytes);

        assert!(events.is_empty(), "parsed {bytes:?} as {events:?}");
    }
}

#[test]
fn non_osc_control_strings_ignore_bel_and_complete_at_st() {
    let bytes = b"\x1bPabc\x07def\x1b\\x";

    let (event, consumed) = extract_one_event(bytes).unwrap();

    assert!(matches!(event, RawInputEvent::Unsupported));
    assert_eq!(consumed, b"\x1bPabc\x07def\x1b\\".len());
}

#[test]
fn non_osc_default_color_text_remains_key_input() {
    let events = parse_raw_input_bytes_sync(b"11;rgb:2828/2a2a/3636\x07");

    assert_eq!(events.len(), 22);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::empty(),
    );
}

#[test]
fn byte_framer_does_not_hold_non_osc_default_color_text() {
    let mut framer = RawInputByteFramer::default();

    let chunks = framer.push(b"11;rgb:2828");

    assert_eq!(chunks.len(), 11);
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn holds_lone_escape_and_stitches_split_host_color_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();

    // The reply is split right at its ESC introducer.
    assert!(framer.push(b"\x1b").is_empty());
    // The idle flush must not release the ESC as an Escape key while a host
    // color reply is still outstanding.
    assert!(framer.flush_timeout().is_empty());

    // The rest of the OSC 11 reply arrives and stitches back together
    // instead of leaking its payload into the focused pane.
    let chunks = framer.push(b"]11;rgb:2424/2727/3a3a\x1b\\");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            ..
        }
    ));
}

fn decoded(chunks: &[Vec<u8>]) -> String {
    let events: Vec<_> = chunks
        .iter()
        .flat_map(|chunk| parse_raw_input_bytes_sync(chunk))
        .collect();
    format!("{events:?}")
}

fn flush_pending(framer: &mut RawInputByteFramer, chunks: &mut Vec<Vec<u8>>) {
    for _ in 0..3 {
        if !framer.has_pending_input() {
            break;
        }
        chunks.extend(framer.flush_timeout());
    }
}

#[test]
fn truncated_host_color_reply_does_not_hold_later_input() {
    // #5052: the link cut an OSC 11 reply and nothing else arrived.
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    assert!(framer.push(b"\x1b]11;rgb:1e1e/1e").is_empty());
    // Like the client loop: idle flushes, then a blocking read.
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    framer.age_partial_host_color_reply(HOST_COLOR_REPLY_IDLE_LIMIT);

    assert_eq!(framer.push(b"echo ok\r").concat(), b"echo ok\r");
    assert!(!framer.has_pending_input());
    assert_eq!(framer.push(b"ls").concat(), b"ls");
}

#[test]
fn idle_flush_drops_host_color_reply_after_idle_limit() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    assert!(framer.push(b"\x1b]11;rgb:1e1e/1e").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.has_pending_input());

    framer.age_partial_host_color_reply(HOST_COLOR_REPLY_IDLE_LIMIT);
    assert!(framer.flush_timeout().is_empty());
    assert!(!framer.has_pending_input());
    // The next read is input, not a reply tail to discard.
    assert_eq!(framer.push(b"echo").concat(), b"echo");
}

#[test]
fn complete_unsupported_default_color_osc_is_consumed() {
    let reply = b"\x1b]11;rgbi:0.5/0.5/0.5\x07";
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    let mut input = reply.to_vec();
    input.push(b'x');
    assert_eq!(framer.push(&input), vec![reply.to_vec(), b"x".to_vec()]);
    assert!(matches!(
        extract_one_event(reply),
        Some((RawInputEvent::Unsupported, _))
    ));
}

#[test]
fn host_color_reply_split_within_idle_limit_assembles() {
    let reply = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\";
    let gap = HOST_COLOR_REPLY_IDLE_LIMIT - Duration::from_millis(100);
    for cut in 1..reply.len() {
        let mut framer = RawInputByteFramer::default();
        framer.host_color_query_sent();
        let mut chunks = framer.push(&reply[..cut]);
        // Once the OSC 10/11 prefix is in, idle flushes keep holding it.
        if cut >= 5 {
            chunks.extend(framer.flush_timeout());
            chunks.extend(framer.flush_timeout());
            framer.age_partial_host_color_reply(gap);
        }
        chunks.extend(framer.push(&reply[cut..]));
        assert_eq!(chunks, vec![reply.to_vec()], "cut at {cut}");
    }
}

#[test]
fn unsolicited_host_color_reply_split_in_three_reads_assembles() {
    let pieces: [&[u8]; 3] = [b"\x1b]11;rgb:1e1e/", b"1e1e/", b"2e2e\x07"];
    let gap = HOST_COLOR_REPLY_IDLE_LIMIT - Duration::from_millis(100);
    let mut framer = RawInputByteFramer::default();
    let mut chunks = Vec::new();
    for piece in pieces {
        chunks.extend(framer.push(piece));
        chunks.extend(framer.flush_timeout());
        framer.age_partial_host_color_reply(gap);
    }
    assert_eq!(chunks, vec![pieces.concat()]);
}

#[test]
fn input_after_dropped_host_color_reply_is_intact_at_every_split() {
    let follow_ups: [&[u8]; 3] = [
        b"\x1b[200~hello\r\x1b[201~",
        b"\x1b[A",
        b"\x1b]4;1;rgb:cdcd/0000/0000\x1b\\",
    ];
    for head in [
        b"\x1b]11;rgb:1e1e/1e".as_slice(),
        b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b",
    ] {
        for follow in follow_ups {
            let expected = decoded(&[follow.to_vec()]);
            for cut in 0..follow.len() {
                let mut framer = RawInputByteFramer::default();
                framer.host_color_query_sent();
                assert!(framer.push(head).is_empty());
                assert!(framer.flush_timeout().is_empty());
                framer.age_partial_host_color_reply(HOST_COLOR_REPLY_IDLE_LIMIT);
                let mut chunks = framer.push(&follow[..cut]);
                chunks.extend(framer.push(&follow[cut..]));
                flush_pending(&mut framer, &mut chunks);
                assert_eq!(
                    decoded(&chunks),
                    expected,
                    "head {head:?} follow {follow:?} cut {cut}"
                );
            }
        }
    }
}

#[test]
fn unfinished_host_color_reply_is_bounded_across_pushes() {
    let prefix = b"\x1b]11;rgb:";
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    assert!(framer.push(prefix).is_empty());
    let mut chunks = Vec::new();
    for _ in 0..200 {
        chunks.extend(framer.push(b"1"));
        assert!(framer.buffer.len() <= MAX_DISCARDED_CONTROL_TAIL_BYTES);
    }
    // Once the buffer reaches the bound it is dropped; later bytes are input.
    let held = MAX_DISCARDED_CONTROL_TAIL_BYTES - prefix.len();
    assert_eq!(chunks.concat(), vec![b'1'; 200 - held]);
}

#[test]
fn holds_lone_escape_and_stitches_split_host_cell_size_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    // The XTWINOPS reply is split right at its ESC introducer.
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());

    let chunks = framer.push(b"[6;21;10t");
    assert_eq!(chunks, vec![b"\x1b[6;21;10t".to_vec()]);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostCellSizeReport {
            width_px: 10,
            height_px: 21,
        }
    ));
}

#[test]
fn timed_out_host_cell_size_reply_fragments_do_not_leak() {
    for (prefix, tail) in [
        (b"\x1b[6".as_slice(), b";21;10t".as_slice()),
        (b"\x1b[6;".as_slice(), b"21;10t".as_slice()),
        (b"\x1b[6;21;".as_slice(), b"10t".as_slice()),
    ] {
        let mut framer = RawInputByteFramer::default();
        framer.host_cell_size_query_sent();

        assert!(framer.push(prefix).is_empty(), "prefix: {prefix:?}");
        assert!(framer.flush_timeout().is_empty(), "prefix: {prefix:?}");
        assert!(framer.push(tail).is_empty(), "tail: {tail:?}");
        assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
    }
}

#[test]
fn split_host_cell_size_reply_after_csi_intro_gets_one_more_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"6;21;10t"), vec![b"\x1b[6;21;10t".to_vec()]);

    let mut alt_bracket = RawInputByteFramer::default();
    alt_bracket.host_cell_size_query_sent();
    assert!(alt_bracket.push(b"\x1b[").is_empty());
    assert!(alt_bracket.flush_timeout().is_empty());
    assert_eq!(alt_bracket.flush_timeout(), vec![b"\x1b[".to_vec()]);
}

#[test]
fn unanswered_host_query_stops_holding_alt_bracket() {
    // Many hosts answer only part of the 256-entry palette query, or no
    // cell size query at all. Once nothing arrives for a while, ESC[ is a
    // key again: a short wait, released alone, next key not swallowed.
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    framer.host_cell_size_query_sent();
    framer.age_host_reply_activity(HOST_REPLY_QUIET_WINDOW);

    assert!(framer.push(b"\x1b[").is_empty());
    assert_eq!(
        framer.idle_flush_timeout_ms(false),
        RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS
    );
    assert_eq!(framer.flush_timeout(), vec![b"\x1b[".to_vec()]);
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
}

#[test]
fn host_reply_burst_keeps_split_protection_alive() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    framer.age_host_reply_activity(HOST_REPLY_QUIET_WINDOW);
    // A reply arriving late still refreshes the window for the next one.
    assert_eq!(
        framer.push(b"\x1b]4;1;rgb:cd/00/00\x1b\\").len(),
        1,
        "palette reply"
    );
    assert!(framer.push(b"\x1b]").is_empty());
    assert_eq!(
        framer.idle_flush_timeout_ms(false),
        INCOMPLETE_SEQUENCE_FLUSH_TIMEOUT_MS
    );
}

#[test]
fn malformed_host_reply_tail_preserves_following_input() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[6;21").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b";10xabc"),
        vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );
}

#[test]
fn host_reply_tail_discard_is_bounded_across_pushes() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[6;21").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(&[b'1'; 64]).is_empty());
    assert_eq!(
        framer.push(&[b'2'; 67]),
        vec![b"2".to_vec(), b"2".to_vec(), b"2".to_vec()]
    );
}

#[test]
fn stops_holding_lone_escape_after_host_cell_size_reply_completes() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert_eq!(
        framer.push(b"\x1b[6;21;10t"),
        vec![b"\x1b[6;21;10t".to_vec()]
    );

    // Window closed: a later lone Escape flushes immediately.
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn default_byte_framer_does_not_rearm_after_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();

    assert_eq!(
        framer.push(GHOSTTY_COLOR_SCHEME_DARK_REPORT),
        vec![GHOSTTY_COLOR_SCHEME_DARK_REPORT.to_vec()]
    );
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn opt_in_does_not_delay_plain_escape_without_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn opted_in_byte_framer_rearms_after_outer_focus_gained() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"[?997;2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn opted_in_byte_framer_reassembles_appearance_reply_split_after_csi() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"?997;2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn opted_in_byte_framer_reassembles_delayed_appearance_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn timed_out_appearance_reply_preserves_pending_color_reply_window() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"2n").is_empty());

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"]10;rgb:aaaa/bbbb/cccc\x1b\\"),
        vec![b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_vec()]
    );
}

#[test]
fn disabled_focus_query_does_not_rearm_byte_framer() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn focus_query_policy_does_not_delay_plain_escape_without_focus() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_appearance_query_on_focus();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn focus_query_without_reply_holds_escape_for_only_one_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn opted_in_byte_framer_rearms_after_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert_eq!(
        framer.push(GHOSTTY_COLOR_SCHEME_DARK_REPORT),
        vec![GHOSTTY_COLOR_SCHEME_DARK_REPORT.to_vec()]
    );

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let chunks = framer.push(b"]10;#abcdef\x07");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Foreground,
            color: RgbColor {
                r: 0xab,
                g: 0xcd,
                b: 0xef
            }
        }
    ));

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let chunks = framer.push(b"]11;#123456\x07");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        }
    ));

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn flushes_lone_escape_when_not_awaiting_host_color_reply() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn gives_up_holding_lone_escape_after_one_idle_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();

    assert!(framer.push(b"\x1b").is_empty());
    // First idle flush holds the escape.
    assert!(framer.flush_timeout().is_empty());
    // No continuation arrived; the second idle flush releases it as Escape.
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn stops_holding_lone_escape_after_host_color_reply_completes() {
    use std::fmt::Write as _;

    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    let mut replies =
        String::from("\x1b]10;rgb:6565/7b7b/8383\x1b\\\x1b]11;rgb:2424/2727/3a3a\x1b\\");
    for index in 0..=u8::MAX {
        let _ = write!(replies, "\x1b]4;{index};rgb:1111/2222/3333\x1b\\");
    }

    let chunks = framer.push(replies.as_bytes());
    assert_eq!(chunks.len(), 258);

    // Window closed: a later lone Escape flushes immediately.
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn overlapping_host_color_queries_keep_holding_split_reply_escape() {
    use std::fmt::Write as _;

    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    framer.host_color_query_sent();
    let mut first_batch =
        String::from("\x1b]10;rgb:6565/7b7b/8383\x1b\\\x1b]11;rgb:2424/2727/3a3a\x1b\\");
    for index in 0..=u8::MAX {
        let _ = write!(first_batch, "\x1b]4;{index};rgb:1111/2222/3333\x1b\\");
    }
    assert_eq!(framer.push(first_batch.as_bytes()).len(), 258);

    // The second batch is still outstanding, so its first reply split at ESC
    // must not leak into the pane as an Escape key.
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let chunks = framer.push(b"]10;rgb:eeee/eeee/eeee\x1b\\");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Foreground,
            ..
        }
    ));
}
