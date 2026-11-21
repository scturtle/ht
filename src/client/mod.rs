//! Core-only client: the UI loop owns host-terminal I/O and presentation, while
//! one adapter worker projects the selected Core into the original shell UI.
//! Sending to the worker and consuming connection failures share one fenced
//! connection; dropping it disconnects the relay, never Core or its PTYs.

mod clipboard_forwarding;
pub(crate) mod connection;
mod core;
mod endpoint_commands;
mod errors;
mod events;
mod frame_output;
#[cfg(test)]
mod frame_output_tests;
mod host_terminal;
mod identity;
mod image_files;
mod input;
mod loop_config;
mod notifications;
mod shell;
mod shell_runtime;
mod state;
mod timer;

use clipboard_forwarding::forward_clipboard;
use events::{ClientLoopEvent, EndpointEvent};
use loop_config::ClientLoopConfig;
use shell_runtime::*;
use state::ClientState;

use host_terminal::{
    current_terminal_geometry, effective_mouse_capture, effective_sgr_pixel_mouse,
    ioctl_terminal_geometry, query_host_terminal_theme, reported_cell_size_from_events,
    resize_poll_loop, set_mouse_capture, setup_terminal, store_reported_cell_size, TerminalGuard,
};

fn refresh_host_mouse_capture(enabled: bool, sgr_pixels: bool) {
    if let Err(err) = set_mouse_capture(enabled, sgr_pixels) {
        warn!(err = %err, "failed to re-assert host mouse capture");
    }
}

fn refresh_host_keyboard_protocol(
    terminal_guard: &TerminalGuard,
    report_all_keys: bool,
    focus_reporting: bool,
) {
    if let Err(err) = terminal_guard.refresh_host_keyboard_protocol(
        &mut io::stdout(),
        report_all_keys,
        focus_reporting,
    ) {
        warn!(err = %err, "failed to re-assert host keyboard protocol");
    }
}

pub use errors::ClientError;
use frame_output::write_encoded_frame_with_graphics;
#[cfg(test)]
use frame_output::{
    clear_received_kitty_graphics, kitty_graphics_image_ids, record_received_kitty_graphics,
};
use notifications::handle_shell_notification_effects;

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::protocol::render_ansi;
use crate::protocol::ClientMessage;

pub fn run_core_client() -> io::Result<()> {
    let core_options = core::Options::from_environment()?;
    crate::logging::init_file_logging("herdr-client.log");

    let loaded_config = crate::config::Config::load();
    host_terminal::clear_host_mouse_reporting(&mut io::stdout())?;
    let shell_config = shell::ClientShellConfig::from_config(&loaded_config.config)
        .with_startup_config_diagnostic(crate::config::config_diagnostic_summary(
            &loaded_config.diagnostics,
        ));

    crate::logging::startup("client");

    // Get one coherent geometry snapshot before raw mode changes the terminal.
    let (cols, rows, cell_width_px, cell_height_px, exact_cell_size) =
        current_terminal_geometry(0, None)?;

    let mut terminal_guard = setup_terminal().map_err(|err| {
        eprintln!("ht: failed to set up terminal: {err}");
        err
    })?;
    let loop_config = ClientLoopConfig {
        mouse_capture_active: true,
        host_escape_disambiguation_active: terminal_guard.host_escape_disambiguation_active(),
        host_sgr_pixel_mouse: terminal_guard.host_sgr_pixel_mouse(),
        initial_host_input: terminal_guard.take_buffered_host_input(),
        host_color_replies_expected: Default::default(),
        shell_config,
    };

    let shell_surface_size = loop_config.shell_config.initial_surface_size(cols, rows);
    info!(socket_override = ?core_options.socket, "connecting to Core");
    let (initial, endpoint_methods) = core::prepare(
        core_options,
        shell_surface_size,
        (cell_width_px, cell_height_px),
        &loaded_config.config,
    )?;

    // Install a panic hook so the foreground client always restores its terminal.
    let panic_restore = terminal_guard.panic_restore();
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        panic_restore();
        original_hook(info);
    }));

    // Create the tokio runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;

    let termination = {
        let _runtime = rt.enter();
        crate::platform::TerminationSignals::new()?
    };

    let mut stdin_reader = None;
    let result = rt.block_on(run_client_loop(
        initial,
        endpoint_methods,
        cols,
        rows,
        cell_width_px,
        cell_height_px,
        exact_cell_size,
        termination,
        loop_config,
        &terminal_guard,
        &mut stdin_reader,
    ));

    // The UI has dropped its receiver, unblocking a backpressured sender.
    // Join the reader before handing stdin back to the outer shell.
    drop(stdin_reader);
    let terminal_restore_failed = terminal_guard.restore().is_err();

    if let Err(err) = &result {
        let _ = writeln!(io::stderr(), "herdr: {err}");
    }
    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("client");

    match result {
        Ok(()) => Ok(()),
        Err(ClientError::ConnectionLost(_)) if terminal_restore_failed => Ok(()),
        Err(_) => std::process::exit(1),
    }
}

/// The main client event loop.
///
/// Uses a threaded architecture:
/// - stdin reader thread → sends raw input bytes to main loop
/// - resize poller thread → sends resize events to main loop
/// - Core adapter thread → publishes typed endpoint events to the main loop
/// - main loop: coordinates input, output, and server communication
async fn run_client_loop(
    initial: Box<core::Session>,
    endpoint_methods: Vec<String>,
    cols: u16,
    rows: u16,
    initial_cell_width_px: u32,
    initial_cell_height_px: u32,
    initial_pixel_geometry_exact: bool,
    mut termination: crate::platform::TerminationSignals,
    mut config: ClientLoopConfig,
    _terminal_guard: &TerminalGuard,
    stdin_reader: &mut Option<input::StdinReader>,
) -> Result<(), ClientError> {
    let should_quit = Arc::new(AtomicBool::new(false));
    let mut state = ClientState {
        blit_encoder: render_ansi::BlitEncoder::new(),
        image_files: image_files::FileTransport::from_environment(),
        mouse_capture_active: config.mouse_capture_active,
        host_color_replies_expected: config.host_color_replies_expected,
        endpoint_mouse_capture_requested: false,
        endpoint_sgr_pixels_requested: false,
        shell_mouse_capture_preference: config.mouse_capture_active,
        pane_keyboard_report_all: false,
        keyboard_report_all_active: false,
        reported_size: (cols, rows),
        reported_cell_size: (initial_cell_width_px, initial_cell_height_px),
        kitty_graphics_enabled: true,
        pixel_geometry_exact: initial_pixel_geometry_exact,
        host_sgr_pixel_mouse: config.host_sgr_pixel_mouse,
        repaint_pending: false,
        detached_process_children: Vec::new(),
        shell: shell::ClientShellState::new(config.shell_config),
    };
    {
        let shell = &mut state.shell;
        shell.set_host_reports_key_releases(config.host_escape_disambiguation_active);
        shell.set_host_erase_byte(crate::platform::terminal_erase_byte());
        shell.set_graphics_cell_size(initial_cell_width_px, initial_cell_height_px);
        shell.set_endpoint_methods(Some(endpoint_methods));
    }
    let host_mouse_capture_active = Arc::new(AtomicBool::new(state.mouse_capture_active));
    // Cell size reported by the host terminal, packed as width<<32 | height.
    // Zero means the host has not reported one.
    let reported_cell_size = Arc::new(AtomicU64::new(0));
    let host_sgr_pixels_active = Arc::new(AtomicBool::new(false));

    // Shared queue for stdin, resize, and Core adapter events.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    let stdin_tx = event_tx.clone();

    let mut endpoint_commands = endpoint_commands::EndpointCommands::default();

    // Spawn the stdin reader thread.
    let stdin_host_color_replies_expected = Arc::clone(&state.host_color_replies_expected);
    // Ask the host directly when ioctl cannot supply pixel geometry.
    let will_query_host_cell_size =
        state.kitty_graphics_enabled && ioctl_terminal_geometry().is_none();
    let stdin_quit = should_quit.clone();
    let stdin_mouse_capture_active = host_mouse_capture_active.clone();
    let stdin_sgr_pixels_active = host_sgr_pixels_active.clone();
    let stdin_escape_disambiguation_active = config.host_escape_disambiguation_active;
    let stdin_initial_host_input = std::mem::take(&mut config.initial_host_input);
    *stdin_reader = Some(input::StdinReader::spawn(should_quit.clone(), move || {
        input::stdin_reader_loop(
            stdin_tx,
            &stdin_quit,
            stdin_host_color_replies_expected,
            will_query_host_cell_size,
            stdin_mouse_capture_active,
            stdin_sgr_pixels_active,
            stdin_escape_disambiguation_active,
            stdin_initial_host_input,
        )
    }));

    query_host_terminal_theme(&state.host_color_replies_expected);

    if will_query_host_cell_size {
        let mut stdout = io::stdout();
        let _ = stdout.write_all(b"\x1b[16t").and_then(|()| stdout.flush());
    }

    // Spawn the resize poller thread.
    let resize_quit = should_quit.clone();
    let resize_tx = event_tx.clone();
    let resize_cell_size = reported_cell_size.clone();
    std::thread::spawn(move || {
        resize_poll_loop(
            resize_tx,
            (
                cols,
                rows,
                initial_cell_width_px,
                initial_cell_height_px,
                initial_pixel_geometry_exact,
            ),
            &resize_cell_size,
            &resize_quit,
        );
    });

    let endpoint = core::start(initial, &event_tx).map_err(ClientError::ConnectionFailed)?;
    let mut connection = connection::EndpointConnection::new(endpoint);
    connection.send(&ClientMessage::ClientShellFocus { focused: true });

    // Main event loop.
    let mut client_timer = timer::ClientLoopTimer::new();
    loop {
        {
            let shell = &mut state.shell;
            let cleanup = shell.take_pending_graphics_cleanup();
            state.present_graphics(&cleanup);
        }
        // Idle sessions wake once a second to reap detached children and
        // service endpoint health and expiring UI state.
        const HOUSEKEEPING_TICK: Duration = Duration::from_secs(1);
        let timer_delay = state
            .shell
            .timer_delay(std::time::Instant::now())
            .unwrap_or(HOUSEKEEPING_TICK);
        let timer_deadline = client_timer.deadline(std::time::Instant::now(), timer_delay);
        let event = tokio::select! {
            biased;
            _ = termination.recv() => break,
            _ = tokio::time::sleep_until(timer_deadline.into()) => ClientLoopEvent::Timer,
            ev = event_rx.recv() => ev.unwrap_or(ClientLoopEvent::Timer),
        };
        let now = std::time::Instant::now();

        match event {
            ClientLoopEvent::StdinInput(data) => {
                if will_query_host_cell_size {
                    let events = crate::raw_input::parse_framed_input(&data);
                    if let Some((width_px, height_px)) = reported_cell_size_from_events(&events) {
                        store_reported_cell_size(&reported_cell_size, width_px, height_px);
                    }
                }
                let events = crate::raw_input::parse_framed_input(&data);
                if crate::raw_input::events_require_host_mode_refresh(&events) {
                    refresh_host_mouse_capture(
                        state.mouse_capture_active,
                        host_sgr_pixels_active.load(Ordering::Acquire),
                    );
                    refresh_host_keyboard_protocol(
                        _terminal_guard,
                        state.keyboard_report_all_active,
                        false,
                    );
                }
                let (outcome, frame) = {
                    let shell = &mut state.shell;
                    let outcome = shell.handle_raw_events(events);
                    let frame = outcome
                        .repaint
                        .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                        .flatten();
                    (outcome, frame)
                };
                if finish_client_shell_input(
                    &mut state,
                    outcome,
                    frame,
                    &mut connection,
                    &mut endpoint_commands,
                )? {
                    return Ok(());
                }
            }
            ClientLoopEvent::PixelMouse(data, geometry) => {
                let (outcome, frame) = {
                    let shell = &mut state.shell;
                    let outcome = shell.handle_pixel_mouse(&data, geometry);
                    let frame = outcome
                        .repaint
                        .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                        .flatten();
                    (outcome, frame)
                };
                if finish_client_shell_input(
                    &mut state,
                    outcome,
                    frame,
                    &mut connection,
                    &mut endpoint_commands,
                )? {
                    return Ok(());
                }
            }
            ClientLoopEvent::TerminalUnavailable(err) => {
                info!(err = %err, "client terminal unavailable; detaching");
                connection.send(&ClientMessage::Detach);
                return Ok(());
            }
            ClientLoopEvent::Resize(
                new_cols,
                new_rows,
                cell_width_px,
                cell_height_px,
                pixel_geometry_exact,
            ) => {
                let pixel_mouse_available =
                    pixel_geometry_exact && state.host_sgr_pixel_mouse == Some(true);
                if !pixel_mouse_available && host_sgr_pixels_active.load(Ordering::Acquire) {
                    set_mouse_capture(state.mouse_capture_active, false)
                        .map_err(ClientError::ConnectionFailed)?;
                    host_sgr_pixels_active.store(false, Ordering::Release);
                } else {
                    refresh_host_mouse_capture(
                        state.mouse_capture_active,
                        host_sgr_pixels_active.load(Ordering::Acquire),
                    );
                }
                // A host that reset its modes (a reconnect or window reload)
                // stops sending focus reports, so a resize is the one
                // dependable trigger to restore them.
                refresh_host_keyboard_protocol(
                    _terminal_guard,
                    state.keyboard_report_all_active,
                    true,
                );
                state.reported_size = (new_cols, new_rows);
                state.reported_cell_size = (cell_width_px, cell_height_px);
                state.pixel_geometry_exact = pixel_geometry_exact;
                // Resizing invalidates both the host-side blit baseline and pane hit geometry.
                state.request_repaint();
                {
                    let shell = &mut state.shell;
                    shell.set_graphics_cell_size(cell_width_px, cell_height_px);
                    shell.invalidate_pane_surface();
                }
                let msg = client_shell_resize_message(
                    &state.shell,
                    new_cols,
                    new_rows,
                    cell_width_px,
                    cell_height_px,
                    pixel_mouse_available,
                );
                connection.send(&msg);
            }
            ClientLoopEvent::Endpoint(event) => {
                if !connection.is_connected() {
                    continue;
                }
                match *event {
                    EndpointEvent::Snapshot(snapshot) => {
                        for id in endpoint_commands.observe_snapshot(&snapshot.boot_id) {
                            state.shell.cancel_endpoint_request(&id);
                        }
                        install_client_shell_snapshot(&mut state, snapshot, &mut connection)?;
                    }
                    EndpointEvent::PaneSurface(surface) => {
                        let composed = {
                            let shell = &mut state.shell;
                            shell.set_pane_surface(surface);
                            shell.compose(state.reported_size.0, state.reported_size.1)
                        };
                        if let Some(frame) = composed {
                            state.present_frame(frame);
                        }
                    }
                    EndpointEvent::TerminalBell { count } => {
                        if let Err(err) =
                            crate::terminal_effects::write_terminal_bells(&mut io::stdout(), count)
                        {
                            warn!(err = %err, "failed to emit terminal bell");
                        }
                    }
                    EndpointEvent::Error { message } => {
                        let shell = &mut state.shell;
                        if shell.receive_endpoint_error(message) {
                            let frame = shell.compose(state.reported_size.0, state.reported_size.1);
                            if let Some(frame) = frame {
                                state.present_frame(frame);
                            }
                        }
                    }
                    EndpointEvent::Response {
                        boot_id,
                        request_id,
                        result,
                    } => {
                        let Some(completed) =
                            endpoint_commands.receive_response(&boot_id, &request_id, result)
                        else {
                            continue;
                        };
                        let (repaint, actions) = state.shell.handle_endpoint_result(
                            &completed.boot_id,
                            &completed.request_id,
                            completed.result,
                        );
                        let (replay_mouse, dispatch_repaint) = dispatch_client_shell_actions(
                            actions,
                            &mut endpoint_commands,
                            &mut connection,
                            &mut state.shell,
                            &mut state.detached_process_children,
                        )?;
                        let repaint = repaint || dispatch_repaint;
                        if replay_mouse.is_empty() {
                            if repaint {
                                if let Some(frame) = state
                                    .shell
                                    .compose(state.reported_size.0, state.reported_size.1)
                                {
                                    state.present_frame(frame);
                                }
                            }
                        } else {
                            let (outcome, frame) = {
                                let shell = &mut state.shell;
                                let mut outcome = shell.replay_mouse_events(replay_mouse);
                                outcome.repaint |= repaint;
                                let frame = outcome
                                    .repaint
                                    .then(|| {
                                        shell.compose(state.reported_size.0, state.reported_size.1)
                                    })
                                    .flatten();
                                (outcome, frame)
                            };
                            if finish_client_shell_input(
                                &mut state,
                                outcome,
                                frame,
                                &mut connection,
                                &mut endpoint_commands,
                            )? {
                                return Ok(());
                            }
                        }
                    }
                    EndpointEvent::Clipboard { data } => {
                        if forward_clipboard(&data) {
                            let (width, height) = state.reported_size;
                            let frame = state
                                .shell
                                .show_copy_feedback(std::time::Instant::now())
                                .then(|| state.shell.compose(width, height))
                                .flatten();
                            if let Some(frame) = frame {
                                state.present_frame(frame);
                            }
                        }
                        let _ = io::stdout().flush();
                    }
                    EndpointEvent::MouseCapture {
                        enabled,
                        sgr_pixels,
                    } => {
                        state.endpoint_mouse_capture_requested = enabled;
                        state.endpoint_sgr_pixels_requested = sgr_pixels;
                        let enabled = state.requested_host_mouse_capture();
                        let next_sgr_pixels = effective_sgr_pixel_mouse(
                            enabled,
                            sgr_pixels,
                            state.pixel_mouse_available(),
                        );
                        let mouse_mode_changed = enabled != state.mouse_capture_active
                            || next_sgr_pixels != host_sgr_pixels_active.load(Ordering::Acquire);
                        if mouse_mode_changed {
                            set_mouse_capture(enabled, next_sgr_pixels)
                                .map_err(ClientError::ConnectionFailed)?;
                        }
                        state.mouse_capture_active = enabled;
                        host_mouse_capture_active.store(enabled, Ordering::Release);
                        host_sgr_pixels_active.store(next_sgr_pixels, Ordering::Release);
                    }
                    EndpointEvent::KeyboardReportAll { enabled } => {
                        state.pane_keyboard_report_all = enabled;
                        sync_client_shell_keyboard_report_all(&mut state)?;
                    }
                    EndpointEvent::Methods { methods } => {
                        install_client_shell_methods(&mut state, methods);
                    }
                }
            }
            ClientLoopEvent::EndpointDetached => {
                if connection.accept_detach() {
                    return Ok(());
                }
            }
            ClientLoopEvent::EndpointFailed => {
                connection.fail(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection was lost",
                ));
            }
            ClientLoopEvent::Timer => {
                client_timer.fired();
                crate::render_prof::flush_if_due();
                state
                    .detached_process_children
                    .retain_mut(|child| child.try_wait().ok().flatten().is_none());
                if let Some(error) = connection.take_failure() {
                    warn!(endpoint = "local", %error, "endpoint transport failed");
                    return Err(ClientError::ConnectionLost(error));
                }
                {
                    let expired_endpoints = endpoint_commands.expire(now);
                    let (effects, outcome, frame) = {
                        let shell = &mut state.shell;
                        let mut outcome = shell.tick_selection_autoscroll(now);
                        for expired in expired_endpoints {
                            let (repaint, actions) = shell.handle_endpoint_result(
                                &expired.boot_id,
                                &expired.request_id,
                                expired.result,
                            );
                            outcome.repaint |= repaint;
                            outcome.actions.extend(actions);
                        }
                        let (effects, notification_repaint) = shell.tick_notifications(now);
                        outcome.repaint |= notification_repaint
                            | shell.tick_copy_feedback(now)
                            | shell.tick_workspace_highlight(now)
                            | shell.tick_endpoint_error(now);
                        let frame = outcome
                            .repaint
                            .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                            .flatten();
                        (effects, outcome, frame)
                    };
                    handle_shell_notification_effects(effects);
                    if finish_client_shell_input(
                        &mut state,
                        outcome,
                        frame,
                        &mut connection,
                        &mut endpoint_commands,
                    )? {
                        return Ok(());
                    }
                }
            }
        }
        if state.repaint_pending {
            if let Some(frame) = state
                .shell
                .compose(state.reported_size.0, state.reported_size.1)
            {
                state.present_frame(frame);
            }
        }
    }

    // A termination signal stops the host input workers and detaches only this UI.
    should_quit.store(true, Ordering::Release);
    connection.send(&ClientMessage::Detach);
    let _ = io::stdout().flush();

    Ok(())
}

#[cfg(test)]
mod tests;
