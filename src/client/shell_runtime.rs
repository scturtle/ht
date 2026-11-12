use super::*;
use std::time::Instant;

/// Adopt the method set of the connection that is serving this client now.
///
/// A recovery probe or replacement Core can advertise a different set, so the
/// menus, settings sections and confirmations are re-gated before the next
/// projection arrives. Returns whether the visible chrome needs a repaint.
pub(super) fn install_client_shell_methods(state: &mut ClientState, methods: Vec<String>) -> bool {
    let changed = state.shell.set_endpoint_methods(Some(methods));
    if changed {
        state.request_repaint();
    }
    changed
}

pub(super) fn dispatch_client_shell_actions(
    actions: Vec<shell::ClientShellAction>,
    commands: &mut endpoint_commands::EndpointCommands,
    endpoints: &mut connection::CoreConnection,
    shell: &mut shell::ClientShellState,
    children: &mut Vec<std::process::Child>,
) -> Result<(Vec<crossterm::event::MouseEvent>, bool), ClientError> {
    let mut replay = Vec::new();
    let mut repaint = false;
    for action in actions {
        match action {
            shell::ClientShellAction::Endpoint { boot_id, request } => {
                if let Some(generation) = endpoints.generation() {
                    commands.enqueue(generation, boot_id, request);
                } else {
                    repaint |= shell.cancel_endpoint_request(&request.id);
                }
            }
            shell::ClientShellAction::DumpPaneDiagnostics {
                pane_id,
                mut client,
            } => {
                let source = endpoints.diagnostic_source(&pane_id, &mut client);
                repaint |= match super::diagnostics::start(pane_id, *client, source) {
                    Ok(path) => shell.diagnostic_notice(
                        &path,
                        Some("Collecting; contains pane screen text. Keep private."),
                    ),
                    Err(error) => shell.diagnostic_error(error.to_string()),
                };
            }
            shell::ClientShellAction::ClipboardWrite(bytes) => {
                if crate::selection::write_osc52_bytes(&bytes) {
                    repaint |= shell.show_copy_feedback(Instant::now());
                }
            }
            shell::ClientShellAction::OpenSafeWebUrl(url) => {
                if crate::client::shell::safe_web_url(&url).is_some() {
                    match crate::platform::open_url(&url) {
                        Ok(Some(child)) => children.push(child),
                        Ok(None) => {}
                        Err(error) => warn!(%error,%url,"failed to open pane URL"),
                    }
                }
            }
            shell::ClientShellAction::ReplayMouse(events) => replay.extend(events),
            shell::ClientShellAction::Keybind(action) => debug!(
                ?action,
                "client shell action awaits its presentation family"
            ),
        }
    }
    for id in commands.send_next(endpoints) {
        repaint |= shell.cancel_endpoint_request(&id);
    }
    Ok((replay, repaint))
}

pub(super) fn client_shell_resize_message(
    shell: &shell::ClientShellState,
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    pixel_mouse: bool,
) -> ClientMessage {
    ClientMessage::ClientShellResize {
        cell_width_px,
        cell_height_px,
        surface_size: shell.surface_size(cols, rows),
        pixel_mouse,
    }
}
pub(super) fn sync_client_shell_keyboard_report_all(
    state: &mut ClientState,
) -> Result<(), ClientError> {
    let desired =
        state.pane_keyboard_report_all || state.shell.host_keyboard_report_all_requested();
    if desired != state.keyboard_report_all_active {
        crate::terminal_modes::set_host_kitty_keyboard_report_all(&mut io::stdout(), desired)
            .map_err(ClientError::ConnectionFailed)?;
        state.keyboard_report_all_active = desired;
        state.shell.set_host_reports_all_keys(desired);
    }
    Ok(())
}
pub(super) fn install_client_shell_snapshot(
    state: &mut ClientState,
    snapshot: Box<crate::protocol::ClientShellSnapshot>,
    endpoints: &mut connection::CoreConnection,
) -> Result<(), ClientError> {
    let Some(generation) = endpoints.generation() else {
        return Ok(());
    };
    let pixels = state.pixel_mouse_available();
    let shell = &mut state.shell;
    let previous = shell.surface_size(state.reported_size.0, state.reported_size.1);
    let epoch_changed = shell.set_endpoint_snapshot_for_generation(generation, snapshot);
    let cleanup = shell.take_pending_graphics_cleanup();
    let next = shell.surface_size(state.reported_size.0, state.reported_size.1);
    let resize = (previous != next).then(|| {
        client_shell_resize_message(
            shell,
            state.reported_size.0,
            state.reported_size.1,
            state.reported_cell_size.0,
            state.reported_cell_size.1,
            pixels,
        )
    });
    let frame = shell.compose(state.reported_size.0, state.reported_size.1);
    if epoch_changed {
        state.request_repaint();
    }
    state.present_graphics(&cleanup);
    if let Some(resize) = resize {
        endpoints.send(&resize);
    }
    if let Some(frame) = frame {
        state.present_frame(frame);
    }
    Ok(())
}
pub(super) fn finish_client_shell_input(
    state: &mut ClientState,
    mut outcome: shell::ClientShellInput,
    frame: Option<super::frame_output::ComposedFrame>,
    endpoints: &mut connection::CoreConnection,
    commands: &mut endpoint_commands::EndpointCommands,
) -> Result<bool, ClientError> {
    if outcome.detach {
        endpoints.send(&ClientMessage::Detach);
        return Ok(true);
    }
    if outcome.resize {
        let resize = client_shell_resize_message(
            &state.shell,
            state.reported_size.0,
            state.reported_size.1,
            state.reported_cell_size.0,
            state.reported_cell_size.1,
            state.pixel_mouse_available(),
        );
        endpoints.send(&resize);
    }
    if outcome.query_host_appearance {
        query_host_terminal_appearance();
    }
    if outcome.query_host_theme {
        query_host_terminal_theme();
    }
    sync_client_shell_keyboard_report_all(state)?;
    for action in &mut outcome.actions {
        if let shell::ClientShellAction::DumpPaneDiagnostics { client, .. } = action {
            client["request_lane"] = commands.diagnostic_snapshot();
            client["client_loop"] = serde_json::json!({"reported_size":state.reported_size,"cell_size":state.reported_cell_size,
                "mouse_capture":state.mouse_capture_active,
                "keyboard_report_all":state.keyboard_report_all_active});
        }
    }
    let (replay, repaint) = dispatch_client_shell_actions(
        outcome.actions,
        commands,
        endpoints,
        &mut state.shell,
        &mut state.detached_process_children,
    )?;
    debug_assert!(
        replay.is_empty(),
        "mouse replay only follows endpoint results"
    );
    let frame = if repaint {
        state
            .shell
            .compose(state.reported_size.0, state.reported_size.1)
    } else {
        frame
    };
    let online = state.shell.endpoint_is_online();
    for request in outcome.requests {
        // Host observations are meaningful before the first snapshot or paint.
        if matches!(&request, ClientMessage::ClientShellHostTheme { .. }) || online {
            endpoints.send(&request);
        }
    }
    if let Some(frame) = frame {
        state.present_frame(frame);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_core_methods_install_once_and_request_one_repaint() {
        let mut state = ClientState::test_new();
        assert!(install_client_shell_methods(
            &mut state,
            vec!["pane.focus".into(), "tab.close".into()]
        ));
        assert!(state.repaint_pending);

        state.repaint_pending = false;
        assert!(!install_client_shell_methods(
            &mut state,
            vec!["tab.close".into(), "pane.focus".into()]
        ));
        assert!(
            !state.repaint_pending,
            "an equal set in another order must not repaint"
        );

        // A replacement Core that implements less re-gates the same shell.
        assert!(install_client_shell_methods(
            &mut state,
            vec!["pane.focus".into()]
        ));
        assert!(state.repaint_pending);
    }

    #[test]
    fn failed_ui_send_leaves_failure_for_the_lifecycle_loop_without_retrying() {
        struct FailedSend(Arc<AtomicU32>);
        impl connection::CoreTransport for FailedSend {
            fn send(&mut self, _: &ClientMessage) -> io::Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "queue closed"))
            }
        }
        let sends = Arc::new(AtomicU32::new(0));
        let mut connection = connection::CoreConnection::new(FailedSend(sends.clone()), 7);
        let mut state = ClientState::test_new();
        let outcome = state.shell.handle_raw_events(vec![
            crate::raw_input::RawInputEvent::HostDefaultColor {
                kind: crate::terminal_theme::DefaultColorKind::Background,
                color: crate::terminal_theme::RgbColor { r: 1, g: 2, b: 3 },
            },
            crate::raw_input::RawInputEvent::HostDefaultColor {
                kind: crate::terminal_theme::DefaultColorKind::Foreground,
                color: crate::terminal_theme::RgbColor { r: 4, g: 5, b: 6 },
            },
        ]);
        assert!(outcome.requests.len() >= 2);
        assert!(
            matches!(
                finish_client_shell_input(
                    &mut state,
                    outcome,
                    None,
                    &mut connection,
                    &mut endpoint_commands::EndpointCommands::default(),
                ),
                Ok(false)
            ),
            "a send failure must follow the normal lifecycle transition"
        );
        assert_eq!(sends.load(Ordering::Relaxed), 1);
        let failure = connection.take_failure().unwrap();
        assert_eq!(failure.generation, 7);
        assert_eq!(failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(failure.message, "queue closed");
        assert!(connection.take_failure().is_none());
    }

    #[test]
    fn host_color_reaches_server_before_first_snapshot() {
        #[derive(Clone)]
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<ClientMessage>>>);

        impl connection::CoreTransport for Capture {
            fn send(&mut self, message: &ClientMessage) -> std::io::Result<()> {
                self.0.lock().unwrap().push(message.clone());
                Ok(())
            }
        }

        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut endpoints = connection::CoreConnection::new(Capture(sent.clone()), 1);
        let mut state = ClientState::test_new();
        assert!(!state.shell.endpoint_is_online());
        let outcome = state.shell.handle_raw_events(vec![
            crate::raw_input::RawInputEvent::HostDefaultColor {
                kind: crate::terminal_theme::DefaultColorKind::Background,
                color: crate::terminal_theme::RgbColor {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
            },
        ]);
        let mut endpoint_commands = endpoint_commands::EndpointCommands::default();
        finish_client_shell_input(
            &mut state,
            outcome,
            None,
            &mut endpoints,
            &mut endpoint_commands,
        )
        .unwrap();

        assert_eq!(
            *sent.lock().unwrap(),
            vec![ClientMessage::ClientShellHostTheme {
                update: crate::protocol::ClientHostThemeUpdate::DefaultColor {
                    kind: crate::protocol::ClientHostDefaultColorKind::Background,
                    color: crate::protocol::ClientHostColor {
                        r: 0x11,
                        g: 0x22,
                        b: 0x33,
                    },
                },
            }]
        );

        let outcome = state.shell.handle_raw_events(vec![
            crate::raw_input::RawInputEvent::HostDefaultColor {
                kind: crate::terminal_theme::DefaultColorKind::Foreground,
                color: crate::terminal_theme::RgbColor { r: 4, g: 5, b: 6 },
            },
        ]);
        finish_client_shell_input(
            &mut state,
            outcome,
            None,
            &mut endpoints,
            &mut endpoint_commands,
        )
        .unwrap();
        assert_eq!(
            sent.lock().unwrap().len(),
            2,
            "theme is forwarded without waiting for a paint"
        );
    }
}
