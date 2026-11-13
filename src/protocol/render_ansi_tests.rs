//! ANSI frame encoder unit tests and test-only blitting helpers.

use super::*;
use crate::protocol::{CellData, CursorState};

/// Blits a frame to a writer, diffing against the previous frame.
fn blit_frame_to(writer: impl Write, frame: &FrameData, prev: Option<&FrameData>) {
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    blit_frame_to_with_cursor_memory(
        writer,
        frame,
        prev,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );
}

fn blit_frame_to_with_cursor_memory(
    writer: impl Write,
    frame: &FrameData,
    prev: Option<&FrameData>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    suppress_visible_cursor: bool,
) {
    blit_frame_to_with_cursor_memory_and_policy(
        writer,
        frame,
        prev,
        last_visible_cursor,
        last_cursor_shape,
        repeat_ime_anchor_after_sync(),
        suppress_visible_cursor,
    );
}

fn blit_frame_to_with_cursor_memory_and_policy(
    writer: impl Write,
    frame: &FrameData,
    prev: Option<&FrameData>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    repeat_ime_anchor: bool,
    suppress_visible_cursor: bool,
) {
    blit_frame_to_with_cursor_memory_and_clear_policy(
        writer,
        frame,
        prev,
        last_visible_cursor,
        last_cursor_shape,
        repeat_ime_anchor,
        true,
        suppress_visible_cursor,
    );
}

const WIDE_GRAPHEME: &str = "💡";
const HALFWIDTH_VOICED_KANA: &str = "ｶ\u{ff9e}";

fn make_cell(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
    CellData {
        symbol: symbol.to_owned(),
        fg,
        bg,
        modifier,
        skip: false,
        hyperlink: None,
    }
}

fn make_skip_cell(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
    let mut cell = make_cell(symbol, fg, bg, modifier);
    cell.skip = true;
    cell
}

fn make_frame(width: u16, height: u16, cells: Vec<CellData>) -> FrameData {
    FrameData {
        cells,
        width,
        height,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

fn linked_cell(symbol: &str, index: u32) -> CellData {
    let mut cell = make_cell(symbol, 0, 0, 0);
    cell.hyperlink = Some(index);
    cell
}

#[test]
fn wide_spacer_contents_cannot_hide_following_changed_cells() {
    // TestBackend retains old symbols underneath the trailing half of a
    // newly drawn wide glyph. Those hidden symbols have no display width.
    let previous = make_frame(
        6,
        1,
        ["a", "b", "c", "d", "e", "f"]
            .map(|s| make_cell(s, 0, 0, 0))
            .to_vec(),
    );
    let next = make_frame(
        6,
        1,
        ["中", "旧", "文", "字", "Z", " "]
            .map(|s| make_cell(s, 0, 0, 0))
            .to_vec(),
    );
    for patch in [false, true] {
        let mut host = crate::ghostty::Terminal::new(6, 1, 0).unwrap();
        host.write(b"\x1b[?7l");
        let mut encoder = BlitEncoder::new();
        let initial = encoder.encode(&previous, false);
        host.write(&initial.bytes);
        encoder.commit(previous.clone(), initial);
        let changed = if patch {
            encoder
                .encode_patch(
                    &[PaneSurfacePatchRow {
                        x: 0,
                        y: 0,
                        cells: next.cells.clone(),
                    }],
                    None,
                    false,
                )
                .unwrap()
        } else {
            encoder.encode(&next, false)
        };
        host.write(&changed.bytes);
        assert_eq!(
            host.read_text_viewport((0, 0), (5, 0), true)
                .unwrap()
                .trim_end(),
            "中文Z",
            "patch={patch}"
        );
    }
}

#[test]
fn repeated_and_equivalent_styles_keep_text_and_link_changes() {
    let mut cells = vec![
        make_cell("a", 0, 0, 0),
        make_cell("b", 0, 0, 0),
        make_cell("c", 0xff_00_00_00, 0, 0), // Unknown color also encodes as reset.
        make_cell("d", 2, 0, 0),
        make_cell("e", 0, 0, 0),
    ];
    cells[1].hyperlink = Some(0);
    let mut frame = make_frame(5, 1, cells);
    frame.hyperlinks.push("https://example.com".into());
    let mut output = Vec::new();
    write_all_cells(&mut output, &frame);
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "\x1b[1;1H\x1b[0;39;49ma\x1b]8;;https://example.com\x1b\\b\x1b]8;;\x1b\\c\x1b[0;31;49md\x1b[0;39;49me\x1b[0m"
    );
}

#[test]
fn build_sgr_preserves_curly_underline_style() {
    let modifier = crate::protocol::modifier_to_u16(
        crate::protocol::modifier_with_underline_style(ratatui::style::Modifier::UNDERLINED, 3),
    );

    assert_eq!(
        build_sgr(0x00_00_00_00, 0x00_00_00_00, modifier),
        "\x1b[0;4:3;39;49m"
    );
}

#[test]
fn blit_frame_hides_cursor_before_full_redraw_writes() {
    let frame = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should hide cursor inside synchronized frame painting during full redraw"
    );
}

#[test]
fn blit_frame_hides_cursor_before_diff_writes() {
    let prev = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    let curr = make_frame(
        2,
        2,
        vec![
            make_cell("X", 0, 0, 0), // Changed
            make_cell("i", 0, 0, 0), // Same
            make_cell("!", 0, 0, 0), // Same
            make_cell(" ", 0, 0, 0), // Same
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should hide cursor inside synchronized frame painting during diff"
    );
}

#[test]
fn blit_frame_wraps_frame_in_synchronized_output() {
    let frame = make_frame(1, 1, vec![make_cell("A", 0, 0, 0)]);

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "should begin synchronized output before frame writes"
    );
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output after frame writes");
    assert!(
        sync_end > 0,
        "should end synchronized output after frame writes"
    );
}

#[test]
fn blit_frame_begins_sync_before_hiding_cursor_after_visible_cursor_repeat() {
    let visible = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut changed = visible.clone();
    changed.cells[0] = make_cell("B", 0, 0, 0);

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut first_output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut first_output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let mut second_output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut second_output,
        &changed,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let second_output_str = std::str::from_utf8(&second_output).unwrap();
    assert!(
        second_output_str.starts_with("\x1b[?2026h\x1b[?25l"),
        "next frame should enter synchronized output before hiding the cursor"
    );

    let hide = second_output_str
        .find("\x1b[?25l")
        .expect("second frame should hide cursor before painting");
    let first_paint = second_output_str
        .find("\x1b[1;1H")
        .expect("second frame should paint changed cell");
    assert!(
        hide < first_paint,
        "cursor should still hide before painting"
    );

    first_output.extend_from_slice(&second_output);
    let combined = String::from_utf8(first_output).unwrap();
    assert!(
        combined.contains("\x1b[?2026l\x1b[2;3H\x1b[?25h\x1b[?2026h\x1b[?25l"),
        "post-sync cursor repeat should be followed by a synchronized cursor hide"
    );
}

#[test]
fn blit_frame_can_repeat_final_cursor_state_after_synchronized_output() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[2;3H\x1b[?25h",
        "should expose only the final cursor state after synchronized output"
    );
}

#[test]
fn blit_frame_can_skip_final_cursor_state_after_synchronized_output() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "",
        "should not expose a post-sync cursor repeat when the target terminal flickers on it"
    );
}

#[test]
fn drawn_cursor_reverses_visible_cursor_cell() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: true,
            shape: 6,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let drawn = frame_with_drawn_cursor(frame.clone());

    assert_eq!(drawn.cells[5].modifier, REVERSED_MODIFIER);
    assert_eq!(frame.cells[5].modifier, 0);

    let encoded = BlitEncoder::new().encode_with_suppressed_visible_cursor(&drawn, false);
    let output_str = String::from_utf8(encoded.bytes).unwrap();

    assert!(
        output_str.contains("\x1b[2;3H\x1b[6 q\x1b[?25l"),
        "drawn cursor mode should park the host cursor hidden at the focused cursor position"
    );
    assert!(
        !output_str.contains("\x1b[?25h"),
        "drawn cursor mode should not show the host cursor"
    );
    assert!(
        output_str.contains("\x1b[0;7;39;49mA"),
        "drawn cursor should be emitted as reverse-video cell content"
    );
}

#[test]
fn drawn_cursor_ignores_hidden_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    assert_eq!(frame_with_drawn_cursor(frame.clone()), frame);
}

#[test]
fn blit_frame_emits_cursor_shape_before_visibility_without_touching_ime_anchor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 6,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let final_cursor = output_str
        .find("\x1b[1;1H\x1b[6 q\x1b[?25h")
        .expect("should set cursor shape before showing cursor");
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    assert!(
        final_cursor < sync_end,
        "shape should be part of the synchronized final cursor state"
    );
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[1;1H\x1b[?25h",
        "IME anchor update should preserve the existing position/visibility-only contract"
    );
}

#[test]
fn blit_frame_repeats_explicit_hidden_cursor_anchor_after_synchronized_output() {
    let visible = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let hidden = FrameData {
        cells: vec![make_cell("B", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 2,
            y: 1,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );
    output.clear();
    blit_frame_to_with_cursor_memory_and_policy(
        &mut output,
        &hidden,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        true,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let sync_end = output_str
        .find("\x1b[?2026l")
        .expect("should end synchronized output");
    let trailing_cursor = &output_str[sync_end + "\x1b[?2026l".len()..];
    assert_eq!(
        trailing_cursor, "\x1b[2;3H\x1b[?25l",
        "should repeat the explicit hidden cursor position while preserving visibility"
    );
}

#[test]
fn blit_frame_emits_osc8_for_linked_cells() {
    let mut frame = make_frame(
        3,
        1,
        vec![
            linked_cell("L", 0),
            linked_cell("i", 0),
            make_cell("!", 0, 0, 0),
        ],
    );
    frame.hyperlinks.push("https://example.com".to_owned());

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("\x1b]8;;https://example.com\x1b\\L"));
    assert!(output_str.contains('i'));
    assert!(output_str.contains("\x1b]8;;\x1b\\"));
}

#[test]
fn blit_frame_sanitizes_hyperlink_uris() {
    let mut frame = make_frame(1, 1, vec![linked_cell("L", 0)]);
    frame
        .hyperlinks
        .push("https://exa\x1b\x07mple.com".to_owned());

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("\x1b]8;;https://example.com\x1b\\L"));
}

#[test]
fn blit_frame_first_frame_produces_output() {
    let frame = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    // Full redraw should start with clear screen.
    assert!(
        output_str.contains("\x1b[2J"),
        "full redraw should clear screen"
    );
    assert!(
        output_str.contains('H') || output_str.contains('i'),
        "should contain cell content"
    );
}

#[test]
fn blit_frame_diff_only_writes_changed_cells() {
    let prev = make_frame(
        2,
        2,
        vec![
            make_cell("H", 0, 0, 0),
            make_cell("i", 0, 0, 0),
            make_cell("!", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
        ],
    );

    // Only the first cell changed.
    let curr = make_frame(
        2,
        2,
        vec![
            make_cell("X", 0, 0, 0), // Changed
            make_cell("i", 0, 0, 0), // Same
            make_cell("!", 0, 0, 0), // Same
            make_cell(" ", 0, 0, 0), // Same
        ],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    let output_str = String::from_utf8(output).unwrap();
    // Diff should NOT clear the screen.
    assert!(
        !output_str.contains("\x1b[2J"),
        "diff should not clear screen"
    );
    // Should contain the changed cell content.
    assert!(output_str.contains('X'), "should contain changed cell 'X'");
}

#[test]
fn scroll_sized_ascii_shift_batches_changed_cells_by_row() {
    const WIDTH: u16 = 140;
    const HEIGHT: u16 = 50;
    let prev = make_frame(
        WIDTH,
        HEIGHT,
        vec![make_cell("A", 0, 0, 0); usize::from(WIDTH) * usize::from(HEIGHT)],
    );
    let curr = make_frame(
        WIDTH,
        HEIGHT,
        vec![make_cell("B", 0, 0, 0); usize::from(WIDTH) * usize::from(HEIGHT)],
    );

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    let cup_count = output.iter().filter(|&&byte| byte == b'H').count();
    assert!(
        cup_count <= usize::from(HEIGHT) + 2,
        "one dense scroll frame should need at most one CUP per row plus cursor anchors, got {cup_count}"
    );
    assert!(
        output.len() <= 16_290,
        "one dense scroll frame should stay below 25% of the 65,161-byte live baseline, got {} bytes",
        output.len()
    );
}

#[test]
fn batched_ascii_diff_replays_to_current_frame() {
    let prev = make_frame(4, 3, vec![make_cell("A", 0, 0, 0); 12]);
    let curr = make_frame(4, 3, vec![make_cell("B", 0, 0, 0); 12]);
    let mut terminal = crate::ghostty::Terminal::new(4, 3, 0).unwrap();

    let mut initial = Vec::new();
    blit_frame_to(&mut initial, &prev, None);
    terminal.write(&initial);

    let mut diff = Vec::new();
    blit_frame_to(&mut diff, &curr, Some(&prev));
    terminal.write(&diff);

    for row in 0..3 {
        for col in 0..4 {
            let (_, graphemes) = terminal.screen_cell(col, row).unwrap();
            assert_eq!(graphemes, vec![u32::from('B')]);
        }
    }
}

#[test]
fn encoder_size_change_repaints_without_clearing() {
    let prev = make_frame(2, 2, vec![make_cell("A", 0, 0, 0); 4]);
    let curr = make_frame(3, 2, vec![make_cell("B", 0, 0, 0); 6]);
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&prev, false);
    encoder.commit(prev, initial);

    let encoded = encoder.encode(&curr, false);
    assert!(encoded.full);
    let output = String::from_utf8(encoded.bytes).unwrap();

    assert!(!output.contains("\x1b[2J"));
    assert!(output.bytes().filter(|byte| *byte == b'B').count() >= 6);
}

#[test]
fn encoder_forced_repaint_writes_all_cells_without_clearing() {
    let frame = make_frame(3, 2, vec![make_cell("A", 0, 0, 0); 6]);
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&frame, false);
    encoder.commit(frame.clone(), initial);

    let encoded = encoder.encode(&frame, true);
    assert!(encoded.full);
    let output = String::from_utf8(encoded.bytes).unwrap();

    assert!(!output.contains("\x1b[2J"));
    assert!(output.bytes().filter(|byte| *byte == b'A').count() >= 6);
}

#[test]
fn retained_patch_matches_full_diff_and_updates_the_encoder_baseline() {
    let previous = make_frame(
        4,
        2,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
            make_cell("d", 0, 0, 0),
            make_cell("e", 0, 0, 0),
            make_cell("f", 0, 0, 0),
            make_cell("g", 0, 0, 0),
            make_cell("h", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&previous, false);
    encoder.commit(previous.clone(), initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 1,
        cells: vec![
            make_cell("E", 0, 0, 0),
            make_cell("f", 0, 0, 0),
            make_cell("G", 0, 0, 0),
            make_cell("h", 0, 0, 0),
        ],
    }];
    let cursor = Some(CursorState {
        x: 3,
        y: 1,
        visible: true,
        shape: 2,
    });
    let mut expected = previous;
    expected.cells[4..8].clone_from_slice(&rows[0].cells);
    expected.cursor = cursor.clone();

    let full_diff = encoder.encode(&expected, false);
    let patch = encoder
        .encode_patch(&rows, cursor.clone(), false)
        .expect("valid retained patch");
    assert_eq!(patch.bytes, full_diff.bytes);
    assert!(encoder.commit_patch(&rows, cursor, patch));
    assert!(encoder.is_current(&expected));
}

#[test]
fn retained_patch_width_transition_matches_full_diff_with_following_cell() {
    let previous = make_frame(
        3,
        1,
        vec![
            make_cell("界", 0, 0, 0),
            make_cell("z", 0, 0, 0),
            make_cell("q", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&previous, false);
    encoder.commit(previous.clone(), initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 0,
        cells: vec![make_cell("x", 0, 0, 0), make_cell("z", 0, 0, 0)],
    }];
    let mut expected = previous;
    expected.cells[0..2].clone_from_slice(&rows[0].cells);

    let full_diff = encoder.encode(&expected, false);
    let patch = encoder
        .encode_patch(&rows, None, false)
        .expect("valid retained patch");
    assert_eq!(patch.bytes, full_diff.bytes);
}

#[test]
fn retained_patch_rejects_overlapping_rows() {
    let frame = make_frame(
        3,
        1,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    );
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode(&frame, false);
    encoder.commit(frame, initial);
    let rows = vec![
        PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: vec![make_cell("A", 0, 0, 0), make_cell("B", 0, 0, 0)],
        },
        PaneSurfacePatchRow {
            x: 1,
            y: 0,
            cells: vec![make_cell("C", 0, 0, 0)],
        },
    ];

    assert!(encoder.encode_patch(&rows, None, false).is_none());
    let mut reversed = rows.clone();
    reversed.reverse();
    assert!(encoder.encode_patch(&reversed, None, false).is_none());

    // Input order need not match screen order; touching runs are disjoint.
    let disjoint = vec![
        PaneSurfacePatchRow {
            x: 2,
            y: 0,
            cells: vec![make_cell("Z", 0, 0, 0)],
        },
        rows[0].clone(),
    ];
    assert!(encoder.encode_patch(&disjoint, None, false).is_some());
}

#[test]
fn metadata_only_patches_do_not_write_but_cursor_changes_do() {
    let mut frame = make_frame(3, 1, vec![make_cell("a", 0, 0, 0); 3]);
    let mut encoder = BlitEncoder::new();
    for cursor in [
        None,
        Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 2,
        }),
        Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 2,
        }),
    ] {
        frame.cursor = cursor.clone();
        let initial = encoder.encode(&frame, false);
        encoder.commit(frame.clone(), initial);
        let encoded = encoder.encode_patch(&[], cursor.clone(), false).unwrap();
        assert!(encoded.bytes.is_empty());
        assert!(encoder.commit_patch(&[], cursor, encoded));
        assert!(encoder.is_current(&frame));
    }
    for cursor in [
        CursorState {
            x: 2,
            y: 0,
            visible: false,
            shape: 2,
        },
        CursorState {
            x: 2,
            y: 0,
            visible: true,
            shape: 2,
        },
    ] {
        let encoded = encoder
            .encode_patch(&[], Some(cursor.clone()), false)
            .unwrap();
        assert!(String::from_utf8_lossy(&encoded.bytes).contains("\x1b[1;3H"));
        assert!(encoder.commit_patch(&[], Some(cursor), encoded));
    }
    // Switching to a client-drawn cursor must still hide the visible host cursor.
    let encoded = encoder
        .encode_patch(
            &[],
            encoder.last_frame.as_ref().unwrap().cursor.clone(),
            true,
        )
        .unwrap();
    assert!(String::from_utf8_lossy(&encoded.bytes).contains("\x1b[?25l"));
    let cursor = encoder.last_frame.as_ref().unwrap().cursor.clone();
    assert!(encoder.commit_patch(&[], cursor.clone(), encoded));
    assert!(encoder
        .encode_patch(&[], cursor.clone(), true)
        .unwrap()
        .bytes
        .is_empty());
    let encoded = encoder.encode_patch(&[], cursor.clone(), false).unwrap();
    assert!(String::from_utf8_lossy(&encoded.bytes).contains("\x1b[?25h"));
    assert!(encoder.commit_patch(&[], cursor.clone(), encoded));
    assert!(encoder
        .encode_patch(&[], cursor, false)
        .unwrap()
        .bytes
        .is_empty());
}

#[test]
fn retained_patch_preserves_the_client_drawn_cursor_overlay() {
    let mut previous = make_frame(
        3,
        1,
        vec![
            make_cell("a", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    );
    previous.cursor = Some(CursorState {
        x: 0,
        y: 0,
        visible: true,
        shape: 0,
    });
    let previous_drawn = frame_with_drawn_cursor(previous.clone());
    let mut encoder = BlitEncoder::new();
    let initial = encoder.encode_with_suppressed_visible_cursor(&previous_drawn, false);
    encoder.commit(previous_drawn, initial);

    let rows = vec![PaneSurfacePatchRow {
        x: 0,
        y: 0,
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell("b", 0, 0, 0),
            make_cell("c", 0, 0, 0),
        ],
    }];
    let cursor = Some(CursorState {
        x: 1,
        y: 0,
        visible: true,
        shape: 0,
    });
    let drawn_rows = encoder
        .patch_rows_with_drawn_cursor(&rows, cursor.as_ref())
        .expect("drawn cursor patch rows");
    let mut expected = previous;
    expected.cells[0..3].clone_from_slice(&rows[0].cells);
    expected.cursor = cursor.clone();
    let expected = frame_with_drawn_cursor(expected);

    let full_diff = encoder.encode_with_suppressed_visible_cursor(&expected, false);
    let patch = encoder
        .encode_patch(&drawn_rows, cursor.clone(), true)
        .expect("valid drawn cursor patch");
    assert_eq!(patch.bytes, full_diff.bytes);
    assert!(encoder.commit_patch(&drawn_rows, cursor, patch));
    assert!(encoder.is_current(&expected));
}

#[test]
fn blit_frame_positions_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[1;1H"),
        "should position cursor at (1,1)"
    );
}

#[test]
fn blit_frame_hides_cursor_when_invisible() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25l"),
        "should hide cursor when invisible"
    );
}

#[test]
fn blit_frame_no_cursor_hides_cursor() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25l"),
        "should hide cursor when no cursor state"
    );
}

#[test]
fn blit_frame_restores_cursor_visibility() {
    // First frame: cursor hidden.
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: false,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &prev, None);
    assert!(
        String::from_utf8(output).unwrap().contains("\x1b[?25l"),
        "first frame should hide cursor"
    );

    // Second frame: cursor visible — should restore visibility.
    let curr = FrameData {
        cells: vec![make_cell("B", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[?25h"),
        "second frame should restore cursor visibility with ?25h"
    );
    assert!(
        output_str.contains("\x1b[1;1H"),
        "should position cursor before showing it"
    );
}

#[test]
fn blit_frame_positions_cursor_before_showing_it() {
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut curr = prev.clone();
    curr.cells[0] = make_cell("B", 0, 0, 0);
    curr.cursor = Some(CursorState {
        x: 2,
        y: 2,
        visible: true,
        shape: 0,
    });

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();
    let final_move = output_str
        .rfind("\x1b[3;3H")
        .expect("should move cursor to final position");
    let show = output_str
        .rfind("\x1b[?25h")
        .expect("should show cursor after positioning it");

    assert!(
        final_move < show,
        "should move cursor to final position before showing it"
    );
}

#[test]
fn blit_frame_parks_hidden_cursor_at_last_visible_position() {
    let visible = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: Some(CursorState {
            x: 1,
            y: 1,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let hidden = FrameData {
        cells: vec![make_cell("B", 0, 0, 0); 9],
        width: 3,
        height: 3,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory(
        &mut output,
        &visible,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );
    output.clear();
    blit_frame_to_with_cursor_memory(
        &mut output,
        &hidden,
        Some(&visible),
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    let park = output_str
        .rfind("\x1b[2;2H")
        .expect("should park hidden cursor at last visible position");
    let hide = output_str
        .rfind("\x1b[?25l")
        .expect("should keep hidden cursor hidden");
    assert!(park < hide, "should park cursor before hiding it");
}

#[test]
fn blit_frame_parks_hidden_cursor_at_bottom_right_without_history() {
    let frame = FrameData {
        cells: vec![make_cell("A", 0, 0, 0); 6],
        width: 3,
        height: 2,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    let mut output = Vec::new();

    blit_frame_to_with_cursor_memory(
        &mut output,
        &frame,
        None,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("\x1b[2;3H\x1b[?25l"),
        "should park hidden cursor at bottom-right before ending the frame"
    );
}

#[test]
fn blit_frame_hides_previous_visible_cursor_when_next_frame_has_none() {
    let prev = FrameData {
        cells: vec![make_cell("A", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![make_cell("B", 0, 0, 0)],
        width: 1,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));

    assert!(
        String::from_utf8(output).unwrap().contains("\x1b[?25l"),
        "diff redraw should hide a previously visible cursor when the next frame has none"
    );
}

#[test]
fn full_redraw_skips_trailing_cells_covered_by_wide_graphemes() {
    let frame = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
    assert!(output_str.contains("\x1b[1;3H"));
}

#[test]
fn full_redraw_skips_trailing_cells_covered_by_halfwidth_voiced_kana() {
    let frame = FrameData {
        cells: vec![
            make_cell(HALFWIDTH_VOICED_KANA, 0, 0, 0),
            make_skip_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &frame, None);
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
    assert!(output_str.contains("\x1b[1;3H"));
}

#[test]
fn diff_redraw_reveals_cells_hidden_by_previous_wide_graphemes() {
    let prev = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(
        output_str.contains("\x1b[1;2H"),
        "cells hidden by a previous wide grapheme must be redrawn when they become visible"
    );
}

#[test]
fn diff_redraw_skips_new_trailing_cells_covered_by_wide_graphemes() {
    let prev = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell("B", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell(WIDE_GRAPHEME, 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(!output_str.contains("\x1b[1;2H"));
}

#[test]
fn diff_redraw_reveals_cells_hidden_by_previous_halfwidth_voiced_kana() {
    let prev = FrameData {
        cells: vec![
            make_cell(HALFWIDTH_VOICED_KANA, 0, 0, 0),
            make_skip_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let curr = FrameData {
        cells: vec![
            make_cell("A", 0, 0, 0),
            make_cell(" ", 0, 0, 0),
            make_cell("Z", 0, 0, 0),
        ],
        width: 3,
        height: 1,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };

    let mut output = Vec::new();
    blit_frame_to(&mut output, &curr, Some(&prev));
    let output_str = String::from_utf8(output).unwrap();

    assert!(output_str.contains("\x1b[1;1H"));
    assert!(
        output_str.contains("\x1b[1;2H"),
        "cells hidden by a previous halfwidth voiced kana must be redrawn when visible"
    );
}
