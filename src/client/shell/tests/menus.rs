use super::*;

/// One frozen menu frame: the full cell grid (symbols and styles), the panel
/// area and the row hits the composition would install.
struct PaintedMenu {
    buffer: Buffer,
    area: Rect,
    rows: Vec<(Rect, usize)>,
}

impl PaintedMenu {
    fn fingerprint(&self) -> u64 {
        let dump = format!("{:?}|{:?}|{:?}", self.buffer, self.area, self.rows);
        dump.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
    }
}

fn decorated_buffer(area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    for cell in &mut buffer.content {
        cell.set_symbol("·").set_style(
            Style::default()
                .fg(ratatui::style::Color::Green)
                .bg(ratatui::style::Color::Blue)
                .add_modifier(Modifier::BOLD | Modifier::DIM | Modifier::UNDERLINED),
        );
    }
    buffer
}

fn painted_global(
    area: Rect,
    decorated: bool,
    launcher: Rect,
    highlighted: usize,
    palette: &Palette,
) -> PaintedMenu {
    let mut buffer = if decorated {
        decorated_buffer(area)
    } else {
        Buffer::empty(area)
    };
    let rendered = render::render_global_menu(
        &mut buffer,
        launcher,
        &ClientGlobalMenuOverlay { highlighted },
        palette,
    )
    .expect("global menu panel");
    PaintedMenu {
        buffer,
        area: rendered.area,
        rows: rendered.menu_rows,
    }
}

fn pane_context_menu(x: u16, y: u16, highlighted: usize) -> ClientContextMenuOverlay {
    ClientContextMenuOverlay {
        methods: None,
        target: ClientContextMenuTarget::Pane {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        },
        x,
        y,
        highlighted,
    }
}

fn tab_context_menu(x: u16, y: u16, methods: &[&str]) -> ClientContextMenuOverlay {
    ClientContextMenuOverlay {
        methods: Some(methods.iter().map(|method| (*method).to_string()).collect()),
        target: ClientContextMenuTarget::Tab {
            tab_id: "tab_1".into(),
            workspace_id: "ws_1".into(),
        },
        x,
        y,
        highlighted: 0,
    }
}

fn painted_context(
    area: Rect,
    decorated: bool,
    menu: &ClientContextMenuOverlay,
    palette: &Palette,
) -> PaintedMenu {
    let mut buffer = if decorated {
        decorated_buffer(area)
    } else {
        Buffer::empty(area)
    };
    let rendered = render::render_context_menu(&mut buffer, menu, palette).expect("context panel");
    PaintedMenu {
        buffer,
        area: rendered.area,
        rows: rendered.menu_rows,
    }
}

#[test]
fn menu_frames_keep_the_existing_cells_styles_and_row_hits() {
    let mut hashes = Vec::new();
    for palette in [Palette::test_palette(), Palette::terminal()] {
        // Global menu: normal screen, first row selected.
        hashes.push(
            painted_global(
                Rect::new(0, 0, 106, 30),
                false,
                Rect::new(0, 24, 40, 1),
                0,
                &palette,
            )
            .fingerprint(),
        );
        // Global menu over a decorated buffer with a middle row selected.
        hashes.push(
            painted_global(
                Rect::new(0, 0, 106, 30),
                true,
                Rect::new(0, 24, 40, 1),
                3,
                &palette,
            )
            .fingerprint(),
        );
        // Global menu on a non-zero origin screen.
        hashes.push(
            painted_global(
                Rect::new(3, 5, 106, 30),
                true,
                Rect::new(3, 29, 40, 1),
                2,
                &palette,
            )
            .fingerprint(),
        );
        // Narrow screen.
        hashes.push(
            painted_global(
                Rect::new(0, 0, 20, 6),
                false,
                Rect::new(0, 4, 10, 1),
                4,
                &palette,
            )
            .fingerprint(),
        );
        // Screen too short for the item list: rows are clipped away, and the
        // highlight is past the end of the list.
        hashes.push(
            painted_global(
                Rect::new(0, 0, 20, 6),
                true,
                Rect::new(0, 0, 10, 1),
                5,
                &palette,
            )
            .fingerprint(),
        );
        // Full pane context menu, first row selected.
        hashes.push(
            painted_context(
                Rect::new(0, 0, 106, 30),
                false,
                &pane_context_menu(10, 8, 0),
                &palette,
            )
            .fingerprint(),
        );
        // Decorated buffer, middle row selected.
        hashes.push(
            painted_context(
                Rect::new(0, 0, 106, 30),
                true,
                &pane_context_menu(10, 8, 3),
                &palette,
            )
            .fingerprint(),
        );
        // Non-zero origin and a pointer sitting on the bottom-right edge.
        hashes.push(
            painted_context(
                Rect::new(3, 5, 106, 30),
                true,
                &pane_context_menu(60, 25, 6),
                &palette,
            )
            .fingerprint(),
        );
        // Every item gated away by capabilities: the panel keeps its minimum width.
        hashes.push(
            painted_context(
                Rect::new(0, 0, 20, 6),
                false,
                &tab_context_menu(0, 5, &["workspace.list"]),
                &palette,
            )
            .fingerprint(),
        );
        // Short screen: the pane menu is taller than the window.
        hashes.push(
            painted_context(
                Rect::new(0, 0, 40, 4),
                true,
                &pane_context_menu(39, 3, 0),
                &palette,
            )
            .fingerprint(),
        );
    }
    // Frozen from the pre-refactor renderers; regenerate only for an explained
    // visual change, never to hide a diff. Only global-menu hashes changed
    // when the settings action was removed; all context-menu hashes stay fixed.
    assert_eq!(
        hashes,
        [
            15644633182556899954,
            10381320392559697235,
            984853612958845479,
            4899042652364715673,
            11504084752447570824,
            14185752707311983027,
            1787853764353740496,
            11095759789957850041,
            12679051088765218619,
            17418703810515401257,
            4575950338187869240,
            18332453669329466153,
            17386366343749310575,
            17246734716826370417,
            12317814179807246960,
            9260004210172304611,
            7523002780273074418,
            1487556178163551511,
            13341582256592687363,
            14855942911886583819,
        ]
    );
}

#[test]
fn menu_rows_draw_exactly_the_rects_they_publish_as_hits() {
    let palette = Palette::test_palette();
    let selected = Style::default()
        .fg(crate::ui::widgets::panel_contrast_fg(&palette))
        .bg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(palette.text).bg(palette.panel_bg);

    // Global menu rows carry one leading space before the label.
    let global = painted_global(
        Rect::new(0, 0, 106, 30),
        false,
        Rect::new(0, 24, 40, 1),
        1,
        &palette,
    );
    let items = global_menu::global_menu_items();
    assert_eq!(global.rows.len(), items.len());
    for (index, (row, hit)) in global.rows.iter().enumerate() {
        assert_eq!(*hit, index);
        assert_eq!(
            *row,
            Rect::new(
                global.area.x + 1,
                global.area.y + 1 + index as u16,
                global.area.width - 2,
                1
            )
        );
        assert_eq!(global.buffer[(row.x, row.y)].symbol(), " ", "row {index}");
        assert_eq!(
            global.buffer[(row.x + 1, row.y)].symbol(),
            &items[index].0[..1],
            "row {index}"
        );
        let style = global.buffer[(row.x, row.y)].style();
        let expected = if index == 1 { selected } else { normal };
        assert_eq!(style.fg, expected.fg, "row {index}");
        assert_eq!(style.bg, expected.bg, "row {index}");
        assert_eq!(
            style.add_modifier.contains(Modifier::BOLD),
            index == 1,
            "row {index}"
        );
    }

    // Context menu rows start flush: no leading space.
    let menu = pane_context_menu(10, 8, 4);
    let labels = menu.items();
    let context = painted_context(Rect::new(0, 0, 106, 30), false, &menu, &palette);
    assert_eq!(context.rows.len(), labels.len());
    for (index, (row, hit)) in context.rows.iter().enumerate() {
        assert_eq!(*hit, index);
        assert_eq!(
            *row,
            Rect::new(
                context.area.x + 1,
                context.area.y + 1 + index as u16,
                context.area.width - 2,
                1
            )
        );
        assert_eq!(
            context.buffer[(row.x, row.y)].symbol(),
            &labels[index].label[..1],
            "row {index}"
        );
        let style = context.buffer[(row.x, row.y)].style();
        let expected = if index == 4 { selected } else { normal };
        assert_eq!(style.fg, expected.fg, "row {index}");
        assert_eq!(style.bg, expected.bg, "row {index}");
        assert_eq!(
            style.add_modifier.contains(Modifier::BOLD),
            index == 4,
            "row {index}"
        );
    }

    // A decorated pane keeps every modifier except the DIM that the panel patch
    // removes; the rows only patch fg/bg/BOLD on top of that.
    let decorated = painted_context(
        Rect::new(0, 0, 106, 30),
        true,
        &pane_context_menu(10, 8, 0),
        &palette,
    );
    let border = &decorated.buffer[(decorated.area.x, decorated.area.y)];
    assert_eq!(border.symbol(), "┌");
    assert_eq!(border.style().fg, Some(palette.accent));
    assert_eq!(border.style().bg, Some(palette.panel_bg));
    assert!(!border.style().add_modifier.contains(Modifier::DIM));
    assert!(border.style().add_modifier.contains(Modifier::UNDERLINED));
    for (row, _) in &decorated.rows {
        let style = decorated.buffer[(row.x, row.y)].style();
        assert!(!style.add_modifier.contains(Modifier::DIM), "{row:?}");
        assert!(style.add_modifier.contains(Modifier::UNDERLINED), "{row:?}");
    }
}

fn global_highlighted(state: &ClientShellState) -> usize {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::GlobalMenu(menu)) => menu.highlighted,
        other => panic!("global menu expected: {other:?}"),
    }
}

fn context_highlighted(state: &ClientShellState) -> usize {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu.highlighted,
        other => panic!("context menu expected: {other:?}"),
    }
}

fn menu_state(width: u16, height: u16) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(width, height).expect("shell frame");
    state
}

#[test]
fn global_menu_keeps_jk_and_the_context_menu_keeps_only_arrow_keys() {
    let mut global = menu_state(106, 30);
    global.toggle_global_menu();
    let last = global_menu::global_menu_items().len() - 1;
    assert_eq!(global_highlighted(&global), 0);

    let down = global.handle_input_bytes(b"j");
    assert!(down.actions.is_empty() && down.requests.is_empty());
    assert_eq!(global_highlighted(&global), 1);
    global.handle_input_bytes(b"j");
    assert_eq!(global_highlighted(&global), 2);
    global.handle_input_bytes(b"k");
    global.handle_input_bytes(b"k");
    global.handle_input_bytes(b"k");
    assert_eq!(
        global_highlighted(&global),
        0,
        "k clamps instead of wrapping"
    );
    for _ in 0..global_menu::global_menu_items().len() + 2 {
        global.handle_input_bytes(b"j");
    }
    assert_eq!(global_highlighted(&global), last);

    // A highlight past the end runs nothing and keeps the menu open.
    global.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: last + 1,
    }));
    let enter = global.handle_input_bytes(b"\r");
    assert!(enter.actions.is_empty() && enter.requests.is_empty());
    assert_eq!(global_highlighted(&global), last + 1);

    let mut context = menu_state(106, 30);
    context.open_pane_context_menu("pane_1".into(), 20, 6);
    let first = context_highlighted(&context);
    let letter = context.handle_input_bytes(b"j");
    assert!(
        letter.actions.is_empty() && letter.requests.is_empty(),
        "the context menu consumes j instead of moving or forwarding it"
    );
    assert_eq!(context_highlighted(&context), first);
    let down = context.handle_input_bytes(b"\x1b[B");
    assert!(down.actions.is_empty() && down.requests.is_empty());
    assert_eq!(context_highlighted(&context), first + 1);
    context.handle_input_bytes(b"\x1b[A");
    assert_eq!(context_highlighted(&context), first);
    let escape = context.handle_input_bytes(b"\x1b");
    assert!(escape.actions.is_empty() && escape.requests.is_empty());
    assert!(context.overlay.is_none());
}

#[test]
fn context_menu_capability_refresh_drops_stale_rows_instead_of_rebinding_them() {
    let mut state = menu_state(106, 30);
    state.set_endpoint_methods(Some(vec![
        "pane.rename".into(),
        "pane.split".into(),
        "pane.zoom".into(),
        "pane.close".into(),
    ]));
    state.open_pane_context_menu("pane_1".into(), 20, 6);
    assert_eq!(context_highlighted(&state), 0);
    state.compose(106, 30).expect("laid out menu rows");
    let stale_row = state.hits.context_menu_rows[0].0;
    assert!(!state.hits.context_menu_rows.is_empty());

    // The re-gated list no longer starts with pane.rename, so the old first row
    // now covers "Split right". Its hits must be gone, not rebound.
    assert!(state.set_endpoint_methods(Some(vec![
        "pane.split".into(),
        "pane.zoom".into(),
        "pane.close".into(),
    ])));
    assert!(state.hits.context_menu_rows.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("the menu stays open across the refresh");
    };
    assert!(menu.items().get(menu.highlighted).is_none());

    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: stale_row.x + 2,
        row: stale_row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        click.actions.is_empty() && click.requests.is_empty(),
        "a stale row must not activate the action that moved under it: {:?}",
        click.actions
    );
    assert!(state.overlay.is_none());
}

#[test]
fn global_menu_items_keep_their_fixed_order_and_actions() {
    use global_menu::ClientGlobalMenuAction as Action;
    assert_eq!(
        global_menu::global_menu_items(),
        [
            (
                "keybinds",
                Action::Binding(crate::input::KeybindAction::Help)
            ),
            (
                "reload config",
                Action::Binding(crate::input::KeybindAction::ReloadConfig)
            ),
            ("dump pane diagnostics", Action::DumpPaneDiagnostics),
            (
                "detach",
                Action::Binding(crate::input::KeybindAction::Detach)
            ),
        ]
    );
}
