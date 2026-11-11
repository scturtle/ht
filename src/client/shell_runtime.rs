use super::*;
use std::time::Instant;

pub(super) fn dispatch_client_shell_actions(
    actions: Vec<shell::ClientShellAction>,
    commands: &mut endpoint_commands::EndpointCommands,
    endpoints: &mut endpoint::EndpointRegistry,
    shell: &mut shell::ClientShellState,
    children: &mut Vec<std::process::Child>,
) -> Result<(Vec<crossterm::event::MouseEvent>, bool), ClientError> {
    let mut replay = Vec::new();
    let mut repaint = false;
    for action in actions {
        match action {
            shell::ClientShellAction::Endpoint {
                endpoint_id,
                boot_id,
                request,
            } => {
                if let Some(connection) = endpoints.connection(&endpoint_id) {
                    commands.enqueue(endpoint_id, connection.generation, boot_id, request);
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
                if crate::app::actions::safe_web_url(&url).is_some() {
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
    for id in commands.send_next(&endpoint::ClientEndpointId::Local, endpoints) {
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
pub(super) fn apply_client_shell_input_source_changes(
    state: &mut ClientState,
    source: &mut impl crate::platform::PrefixInputSource,
) {
    for active in state.shell.take_input_source_changes() {
        if active {
            source.switch_to_ascii();
        } else {
            source.restore();
        }
    }
}
pub(super) fn install_client_shell_snapshot(
    state: &mut ClientState,
    id: &endpoint::ClientEndpointId,
    snapshot: Box<crate::protocol::ClientShellSnapshot>,
    endpoints: &mut endpoint::EndpointRegistry,
    input_source: &mut impl crate::platform::PrefixInputSource,
) -> Result<(), ClientError> {
    let Some(connection) = endpoints.connection(id) else {
        return Ok(());
    };
    let generation = connection.generation;
    let pixels = state.pixel_mouse_available();
    let shell = &mut state.shell;
    let previous = shell.surface_size(state.reported_size.0, state.reported_size.1);
    shell.set_endpoint_status(id, endpoint::ClientEndpointStatus::Online);
    let epoch_changed = shell.set_endpoint_snapshot_for_generation(id, generation, snapshot);
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
    apply_client_shell_input_source_changes(state, input_source);
    state.present_graphics(&cleanup);
    if let Some(resize) = resize {
        endpoints.send_to(id, &resize);
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
    endpoints: &mut endpoint::EndpointRegistry,
    commands: &mut endpoint_commands::EndpointCommands,
    input_source: &mut impl crate::platform::PrefixInputSource,
) -> Result<bool, ClientError> {
    apply_client_shell_input_source_changes(state, input_source);
    if outcome.detach {
        let _ = write_to_server(endpoints, &ClientMessage::Detach);
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
        write_to_server(endpoints, &resize).map_err(ClientError::ConnectionLost)?;
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
    let online = state.shell.endpoint_is_online(endpoints.active_id());
    for request in outcome.requests {
        // Host observations are meaningful before the first snapshot or paint.
        if matches!(&request, ClientMessage::ClientShellHostTheme { .. }) || online {
            write_to_server(endpoints, &request).map_err(ClientError::ConnectionLost)?;
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
    fn host_color_reaches_server_before_first_snapshot() {
        #[derive(Clone)]
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<ClientMessage>>>);

        impl endpoint::EndpointTransport for Capture {
            fn send(&mut self, message: &ClientMessage) -> std::io::Result<()> {
                self.0.lock().unwrap().push(message.clone());
                Ok(())
            }
        }

        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut endpoints = endpoint::EndpointRegistry::new(Capture(sent.clone()), 1);
        let mut state = ClientState::test_new();
        assert!(!state
            .shell
            .endpoint_is_online(&endpoint::ClientEndpointId::Local));
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
        let mut prefix_input_source = crate::platform::RealPrefixInputSource::default();
        finish_client_shell_input(
            &mut state,
            outcome,
            None,
            &mut endpoints,
            &mut endpoint_commands,
            &mut prefix_input_source,
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
            &mut prefix_input_source,
        )
        .unwrap();
        assert_eq!(
            sent.lock().unwrap().len(),
            2,
            "theme is forwarded without waiting for a paint"
        );
    }
}
