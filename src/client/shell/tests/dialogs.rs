use super::*;

fn dialog(rename: bool) -> ClientShellOverlay {
    if rename {
        ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename pane",
            input: TextEditor::from("中文 e\u{301} / a long pane label"),
            target: ClientRenameTarget::Pane {
                pane_id: "pane_1".into(),
            },
        })
    } else {
        ClientShellOverlay::ConfirmClose(ClientConfirmCloseOverlay {
            workspace_id: "ws_1".into(),
            close_group: false,
            tab_target: None,
            title: "Close workspace?".into(),
            detail: "中文 workspace · 1 pane".into(),
        })
    }
}

struct PaintedDialog {
    buffer: Buffer,
    area: Rect,
    buttons: [Rect; 3],
    cursor: Option<crate::protocol::CursorState>,
}

fn painted_dialog(area: Rect, rename: bool, palette: &Palette) -> PaintedDialog {
    let state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut buffer = Buffer::empty(area);
    for cell in &mut buffer.content {
        cell.set_symbol("·").set_style(
            Style::default()
                .fg(ratatui::style::Color::Green)
                .bg(ratatui::style::Color::Blue)
                .add_modifier(Modifier::BOLD | Modifier::DIM | Modifier::UNDERLINED),
        );
    }
    let rendered = render::render_client_overlay(
        &mut buffer,
        &dialog(rename),
        &state.endpoint,
        &state.config.keybinds,
        palette,
    )
    .expect("dialog fits");
    PaintedDialog {
        buffer,
        area: rendered.area,
        buttons: [rendered.primary, rendered.clear, rendered.cancel],
        cursor: rendered.cursor,
    }
}

#[test]
fn dialog_frames_keep_the_existing_cells_styles_cursor_and_hit_geometry() {
    let mut hashes = Vec::new();
    for palette in [Palette::catppuccin(), Palette::terminal()] {
        for rename in [true, false] {
            for area in [
                Rect::new(0, 0, 106, 24),
                Rect::new(0, 0, 44, 20),
                Rect::new(0, 0, 38, 8),
                Rect::new(3, 5, 106, 24),
            ] {
                let frame = painted_dialog(area, rename, &palette);
                let dump = format!(
                    "{:?}|{:?}|{:?}|{:?}",
                    frame.buffer, frame.area, frame.buttons, frame.cursor
                );
                let hash = dump.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                });
                hashes.push(hash);
            }
        }
    }
    // Frozen from the original renderer before introducing shared components.
    assert_eq!(
        hashes,
        [
            3152207895608538382,
            4262189316841218858,
            11207147721042957011,
            14835894153605353102,
            16765686734684489513,
            16497384065344462962,
            17879701443184892061,
            10846760539583969711,
            10304178282581641725,
            15178525018226368883,
            2797883840851316503,
            10652650006660465643,
            7081160382213901878,
            14527098852265108599,
            14696534503459803194,
            13388634360177505542,
        ]
    );
}

#[test]
fn shared_dialog_button_hits_keep_rename_actions_local_and_targeted() {
    for (width, height) in [(106, 24), (38, 8)] {
        for button in 0..3 {
            let mut state =
                ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
            state.set_snapshot(Box::new(snapshot()));
            state.set_pane_surface(surface());
            state.open_rename_pane_overlay();
            if let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_mut() {
                rename.input = TextEditor::from(" 新名字 ");
            }
            state.compose(width, height).unwrap();
            let rect = [
                state.hits.overlay_primary,
                state.hits.overlay_clear,
                state.hits.overlay_cancel,
            ][button];
            assert!(!rect.is_empty());
            let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(
                outcome.requests.is_empty(),
                "dialog mouse input must not leak to the pane"
            );
            match button {
                0 => {
                    assert!(
                        matches!(outcome.actions.as_slice(), [ClientShellAction::Endpoint { request, .. }]
                        if matches!(&request.action, crate::actions::Action::PaneRename(params)
                            if params.pane_id == "pane_1" && params.label.as_deref() == Some("新名字")))
                    );
                    assert!(state.overlay.is_none());
                }
                1 => {
                    assert!(outcome.actions.is_empty());
                    assert!(
                        matches!(state.overlay.as_ref(), Some(ClientShellOverlay::Rename(rename)) if rename.input.is_empty())
                    );
                }
                _ => {
                    assert!(outcome.actions.is_empty());
                    assert!(state.overlay.is_none());
                }
            }
        }
    }
}

#[test]
fn help_frames_keep_existing_search_wrapping_scroll_cursor_and_hits() {
    let state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut hashes = Vec::new();
    for palette in [Palette::catppuccin(), Palette::terminal()] {
        for decorated in [false, true] {
            let mut hash = 0xcbf29ce484222325_u64;
            for (query, search_focused, scroll) in [
                ("", false, 0),
                ("", true, usize::MAX),
                ("pane", true, 3),
                ("pane", false, usize::MAX),
                ("中文 e\u{301} / a long filter", true, 0),
                ("missing-command", false, 0),
            ] {
                for area in [
                    Rect::new(0, 0, 106, 30),
                    Rect::new(0, 0, 50, 18),
                    Rect::new(0, 0, 26, 10),
                    Rect::new(0, 0, 24, 10),
                    Rect::new(0, 0, 106, 9),
                    Rect::new(3, 5, 106, 30),
                ] {
                    let mut buffer = Buffer::empty(area);
                    if decorated {
                        for cell in &mut buffer.content {
                            cell.set_symbol("·").set_style(
                                Style::default()
                                    .fg(ratatui::style::Color::Green)
                                    .bg(ratatui::style::Color::Blue)
                                    .add_modifier(
                                        Modifier::BOLD | Modifier::DIM | Modifier::UNDERLINED,
                                    ),
                            );
                        }
                    }
                    let help = ClientShellOverlay::Help(ClientHelpOverlay {
                        query: TextEditor::from(query),
                        search_focused,
                        scroll,
                    });
                    let rendered = render::render_client_overlay(
                        &mut buffer,
                        &help,
                        &state.endpoint,
                        &state.config.keybinds,
                        &palette,
                    )
                    .map(|frame| {
                        (
                            frame.area,
                            frame.cancel,
                            frame.help_popup,
                            frame.help_scrollbar,
                            frame.help_scroll_metrics,
                            frame.help_max_scroll,
                            frame.cursor,
                        )
                    });
                    for byte in format!("{buffer:?}|{rendered:?}").bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                    }
                }
            }
            hashes.push(hash);
        }
    }
    // 144 frames including wrapping, clamped scroll, Unicode filter and rejection.
    // Intentionally refreshed when removing the retired toggle-sidebar help row;
    // the shared dialog painting/layout is unchanged.
    assert_eq!(
        hashes,
        [
            14077277466857360920,
            15801730575480443467,
            15413754114629691577,
            15247371855747336331,
        ]
    );
}

#[test]
fn help_mouse_scroll_and_header_back_keep_input_local() {
    for (width, height) in [(106, 30), (50, 18), (26, 10)] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.mobile_width_threshold = 0;
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface());
        state.overlay = Some(ClientShellOverlay::Help(ClientHelpOverlay {
            query: TextEditor::default(),
            search_focused: false,
            scroll: 0,
        }));
        state.compose(width, height).unwrap();
        let mouse = |state: &mut ClientShellState, kind, x, y| {
            let result = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(result.requests.is_empty() && result.actions.is_empty());
        };
        let track = state.hits.help_scrollbar;
        assert!(!track.is_empty());
        mouse(&mut state, MouseEventKind::ScrollDown, track.x, track.y);
        assert!(
            matches!(state.overlay, Some(ClientShellOverlay::Help(ClientHelpOverlay {scroll, ..})) if scroll > 0)
        );
        state.compose(width, height).unwrap();
        if track.height > 1 {
            let thumb =
                crate::ui::scrollbar_thumb(state.hits.help_scroll_metrics.unwrap(), track).unwrap();
            mouse(
                &mut state,
                MouseEventKind::Down(MouseButton::Left),
                track.x,
                thumb.top,
            );
            mouse(
                &mut state,
                MouseEventKind::Drag(MouseButton::Left),
                track.x,
                track.bottom() - 1,
            );
            mouse(
                &mut state,
                MouseEventKind::Up(MouseButton::Left),
                track.x,
                track.bottom() - 1,
            );
            assert!(state.chrome_drag.is_none());
            assert!(
                matches!(state.overlay, Some(ClientShellOverlay::Help(ClientHelpOverlay {scroll, ..})) if scroll == state.hits.help_max_scroll)
            );
        }
        for bytes in [b"/".as_slice(), b"pane"] {
            let result = state.handle_input_bytes(bytes);
            assert!(result.requests.is_empty() && result.actions.is_empty());
        }
        let frame = state.compose(width, height).unwrap();
        assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));
        let back = state.hits.overlay_cancel;
        mouse(
            &mut state,
            MouseEventKind::Down(MouseButton::Left),
            back.x,
            back.y,
        );
        assert!(
            matches!(&state.overlay, Some(ClientShellOverlay::Help(help))
            if !help.search_focused && help.query.is_empty() && help.scroll == 0)
        );
        state.compose(width, height).unwrap();
        let close = state.hits.overlay_cancel;
        mouse(
            &mut state,
            MouseEventKind::Down(MouseButton::Left),
            close.x,
            close.y,
        );
        assert!(state.overlay.is_none());
    }
}

#[test]
fn settings_frames_keep_existing_content_styles_and_hit_geometry() {
    let state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut hashes = Vec::new();
    for palette in [Palette::catppuccin(), Palette::terminal()] {
        for decorated in [false, true] {
            for section in ClientSettingsSection::ALL {
                let mut hash = 0xcbf29ce484222325_u64;
                let count = match section {
                    ClientSettingsSection::Theme => crate::config::THEME_NAMES.len(),
                    ClientSettingsSection::Toast => 4,
                    _ => 2,
                };
                for selected in [0, count - 1] {
                    for area in [
                        Rect::new(0, 0, 106, 30),
                        Rect::new(0, 0, 50, 18),
                        Rect::new(0, 0, 26, 12),
                        Rect::new(0, 0, 24, 12),
                        Rect::new(0, 0, 106, 11),
                        Rect::new(3, 5, 106, 30),
                    ] {
                        let mut buffer = Buffer::empty(area);
                        if decorated {
                            for cell in &mut buffer.content {
                                cell.set_symbol("·").set_style(
                                    Style::default()
                                        .fg(ratatui::style::Color::Green)
                                        .bg(ratatui::style::Color::Blue)
                                        .add_modifier(
                                            Modifier::BOLD | Modifier::DIM | Modifier::UNDERLINED,
                                        ),
                                );
                            }
                        }
                        let settings = ClientSettingsOverlay {
                            sections: ClientSettingsSection::ALL.to_vec(),
                            section: *section,
                            selected,
                            original_theme_name: "catppuccin".into(),
                            original_palette: palette.clone(),
                        };
                        let rendered = render::render_client_overlay(
                            &mut buffer,
                            &ClientShellOverlay::Settings(settings),
                            &state.endpoint,
                            &state.config.keybinds,
                            &palette,
                        )
                        .map(|frame| {
                            (
                                frame.area,
                                frame.primary,
                                frame.cancel,
                                frame.settings_popup,
                                frame.settings_tabs,
                                frame.settings_choices,
                                frame.cursor,
                            )
                        });
                        for byte in format!("{buffer:?}|{rendered:?}").bytes() {
                            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                        }
                    }
                }
                hashes.push(hash);
            }
        }
    }
    // Frozen before extending the shared layout to settings: 192 frames,
    // including blank/decorated buffers and panels too small to return hits.
    assert_eq!(
        hashes,
        [
            12356087409755544764,
            14569266705724263125,
            1861271064629599949,
            2180137444135004661,
            9040510970419154112,
            12496798520995091103,
            16699270273318693841,
            8270832477339461806,
            409979764271441072,
            14949807614739535369,
            17364408685117915201,
            13212695820046242531,
            11696012028769703260,
            14177559935291189421,
            5628352444209431459,
            9815596427929989456,
        ]
    );
}

#[test]
fn settings_mouse_selection_and_close_keep_preview_and_input_local() {
    for (width, height) in [(106, 30), (50, 18), (26, 12)] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.mobile_width_threshold = 0;
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface());
        let original_theme = state.config.theme_name.clone();
        state.open_settings_overlay();
        let click = |state: &mut ClientShellState, rect: Rect| {
            assert!(!rect.is_empty());
            let result = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x,
                row: rect.y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(result.requests.is_empty() && result.actions.is_empty());
        };
        state.compose(width, height).unwrap();
        let (rect, index) = *state.hits.settings_choices.last().unwrap();
        click(&mut state, rect);
        assert_eq!(state.config.theme_name, crate::config::THEME_NAMES[index]);
        state.compose(width, height).unwrap();
        let close = state.hits.overlay_cancel;
        click(&mut state, close);
        assert!(state.overlay.is_none());
        assert_eq!(state.config.theme_name, original_theme);

        state.open_settings_overlay();
        state.compose(width, height).unwrap();
        let section = state
            .hits
            .settings_tabs
            .iter()
            .find(|(_, section)| *section == ClientSettingsSection::Sound)
            .copied();
        if width == 26 {
            assert!(
                section.is_none(),
                "clipping must not promote the hidden sound tab"
            );
        } else {
            let (rect, _) = section.expect("sound tab is visible");
            click(&mut state, rect);
            state.compose(width, height).unwrap();
            assert_eq!(state.hits.settings_choices.len(), 2);
            // Indicators, Sound and Toast apply immediately on a choice click,
            // which writes the config file. That write is covered by the private
            // organization TUI gate; keep this test on the non-writing section
            // switch and keyboard selection so it needs no config path of its own.
            let selected = match &state.overlay {
                Some(ClientShellOverlay::Settings(settings))
                    if settings.section == ClientSettingsSection::Sound =>
                {
                    settings.selected
                }
                _ => panic!("sound section stays open after a tab click"),
            };
            let away = if selected == 0 {
                b"\x1b[B".as_slice()
            } else {
                b"\x1b[A".as_slice()
            };
            let moved = state.handle_input_bytes(away);
            assert!(moved.requests.is_empty() && moved.actions.is_empty());
            assert!(
                matches!(&state.overlay, Some(ClientShellOverlay::Settings(settings))
                if settings.section == ClientSettingsSection::Sound
                    && settings.selected == 1 - selected)
            );
        }
        let escape = state.handle_input_bytes(b"\x1b");
        assert!(escape.requests.is_empty() && escape.actions.is_empty());
        assert!(state.overlay.is_none());
    }
}

#[test]
fn short_rename_dialog_does_not_put_the_editor_cursor_on_a_button_or_border() {
    for height in [6, 7] {
        let frame = painted_dialog(Rect::new(0, 0, 44, height), true, &Palette::catppuccin());
        assert!(
            frame.cursor.is_none(),
            "short viewport has no separate input row"
        );
        let bottom = frame.area.bottom() - 1;
        for x in frame.area.x + 1..frame.area.right() - 1 {
            assert_eq!(
                frame.buffer[(x, bottom)].symbol(),
                "─",
                "border at {x},{bottom}"
            );
        }
    }
}
