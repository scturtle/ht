//! Core-only client: the UI loop owns host-terminal I/O and presentation, while
//! one adapter worker projects the selected Core into the original shell UI.
//! Sending to the worker and consuming connection failures share one fenced
//! connection; dropping it disconnects the relay, never Core or its PTYs.

mod clipboard_forwarding;
mod config_reload;
pub(crate) mod connection;
mod core;
mod diagnostics;
mod direct_graphics;
mod endpoint_commands;
mod errors;
mod events;
mod frame_output;
#[cfg(test)]
mod frame_output_tests;
mod identity;
mod image_files;
mod input;
mod loop_config;
mod notifications;
mod shell;
mod shell_runtime;
mod state;
mod terminal_geometry;
#[cfg(test)]
mod terminal_sessions;
mod terminal_setup;
mod timer;

#[cfg(test)]
use clipboard_forwarding::decode_clipboard_payload;
use clipboard_forwarding::forward_clipboard;
#[cfg(test)]
use config_reload::reload_local_client_config;
use config_reload::{apply_reload, init_logging};
use events::ClientLoopEvent;
use loop_config::ClientLoopConfig;
use shell_runtime::*;
use state::ClientState;
use state::RetiredDirectGraphicsMatch;

use terminal_geometry::query_host_terminal_appearance;
#[cfg(test)]
use terminal_geometry::{
    cell_size_fallback, current_terminal_geometry_with, ioctl_cell_size, pack_cell_size,
    resize_report_required, should_query_host_cell_size, write_host_cell_size_query,
    write_host_terminal_appearance_query, write_host_terminal_theme_query,
};
use terminal_geometry::{
    host_cell_size_query_required, initial_terminal_geometry, query_host_cell_size,
    query_host_terminal_theme, resize_poll_loop,
};
use terminal_geometry::{reported_cell_size_from_events, store_reported_cell_size};
use terminal_setup::{
    effective_mouse_capture, effective_sgr_pixel_mouse, set_mouse_capture, setup_terminal,
    should_draw_host_cursor, TerminalGuard,
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

#[cfg(test)]
use terminal_setup::{
    windows_virtual_terminal_input_mode, windows_win32_input_mode_enabled,
    write_host_color_scheme_report_mode, write_terminal_restore_postlude,
};

pub use errors::ClientError;
#[cfg(test)]
use frame_output::{clear_received_kitty_graphics, kitty_graphics_image_ids};
use frame_output::{
    contains_kitty_graphics_bytes, record_received_kitty_graphics,
    write_encoded_frame_with_graphics,
};
use notifications::handle_shell_notification_effects;
#[cfg(test)]
use notifications::{handle_notify_with_notifiers, sound_from_notify_message};
#[cfg(test)]
use terminal_sessions::terminal_control_command_from_json;

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::protocol::render_ansi;
#[cfg(test)]
use crate::protocol::{AttachScrollDirection, AttachScrollSource, NotifyKind};
use crate::protocol::{ClientMessage, ServerMessage};

pub fn run_core_client() -> io::Result<()> {
    let core_options = core::Options::from_environment()?;
    init_logging();

    let loaded_config = crate::config::Config::load();
    crate::terminal_modes::clear_host_mouse_reporting(&mut io::stdout())?;
    // Keep the existing chrome preference key; it is not a transport socket.
    let preference_key = crate::config::config_dir().join("herdr-client.sock");
    let shell_config = shell::ClientShellConfig::from_config(&loaded_config.config)
        .with_startup_config_diagnostic(crate::config::config_diagnostic_summary(
            &loaded_config.diagnostics,
        ))
        .with_process_endpoint_preferences(&preference_key);
    let mouse_capture = loaded_config.config.ui.mouse_capture;
    let redraw_on_focus_gained = loaded_config.config.ui.redraw_on_focus_gained;
    let host_cursor = loaded_config.config.ui.host_cursor;
    let kitty_graphics_enabled = loaded_config.config.kitty_graphics_enabled();
    // Pane images are parsed and drawn by this client now, so it owns the flag
    // the retired server used to set from its own configuration.
    crate::kitty_graphics::set_enabled(kitty_graphics_enabled);
    let pixel_geometry_enabled = kitty_graphics_enabled;
    let mut loop_config = ClientLoopConfig {
        sound_config: loaded_config.config.ui.sound.clone(),
        redraw_on_focus_gained,
        host_cursor,
        kitty_graphics_enabled,
        pixel_geometry_enabled,
        pixel_geometry_fallback: kitty_graphics_enabled,
        mouse_capture_active: mouse_capture,
        host_escape_disambiguation_active: false,
        host_sgr_pixel_mouse: None,
        initial_host_input: Vec::new(),
        shell_config,
    };

    crate::logging::startup("client");
    info!(path = %core_options.socket.display(), "connecting to Core");

    // Get one coherent geometry snapshot before raw mode changes the terminal.
    let (cols, rows, cell_width_px, cell_height_px, exact_cell_size) =
        initial_terminal_geometry(pixel_geometry_enabled, kitty_graphics_enabled)?;

    let mut terminal_guard = setup_terminal(mouse_capture).map_err(|err| {
        eprintln!("ht: failed to set up terminal: {err}");
        err
    })?;
    loop_config.host_escape_disambiguation_active =
        terminal_guard.host_escape_disambiguation_active();
    loop_config.host_sgr_pixel_mouse = terminal_guard.host_sgr_pixel_mouse();
    loop_config.initial_host_input = terminal_guard.take_buffered_host_input();

    let shell_surface_size = loop_config.shell_config.initial_surface_size(cols, rows);
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

    let should_quit = Arc::new(AtomicBool::new(false));

    // ctrlc's "termination" feature also catches SIGTERM/SIGHUP so direct
    // termination signals still run the quit path and TerminalGuard::Drop.
    let quit_flag = should_quit.clone();
    if let Err(err) = ctrlc::set_handler(move || {
        quit_flag.store(true, Ordering::Release);
    }) {
        warn!(%err, "failed to install termination handler; terminal restore relies on TerminalGuard::Drop and the panic hook");
    }

    let result = rt.block_on(async {
        run_client_loop(
            initial,
            endpoint_methods,
            cols,
            rows,
            cell_width_px,
            cell_height_px,
            exact_cell_size,
            should_quit,
            loop_config,
            &terminal_guard,
        )
        .await
    });

    // Restore the terminal before printing any final status message.
    let terminal_restore_failed = terminal_guard.restore().is_err();

    if let Err(err) = result {
        let _ = writeln!(io::stderr(), "herdr: {err}");
        rt.shutdown_timeout(Duration::from_millis(100));
        crate::logging::shutdown("client");

        let detached = matches!(
            &err,
            ClientError::ServerShutdown {
                reason: Some(reason)
            } if reason == "detached"
        );
        let connection_lost_during_terminal_hangup =
            terminal_restore_failed && matches!(&err, ClientError::ConnectionLost(_));
        if detached || connection_lost_during_terminal_hangup {
            return Ok(());
        }

        std::process::exit(1);
    }

    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("client");
    Ok(())
}

/// A transport failure whose generation a live connection already replaced must not
/// end the loop; every other failure is still fatal. Checked before the Timer work
/// below runs so an ignored failure never skips it.
fn failure_is_superseded(endpoints: &connection::CoreConnection, failure_generation: u64) -> bool {
    endpoints
        .generation()
        .is_some_and(|generation| generation != failure_generation)
}

/// The main client event loop.
///
/// Uses a threaded architecture:
/// - stdin reader thread → sends raw input bytes to main loop
/// - resize poller thread → sends resize events to main loop
/// - server reader thread → reads ServerMessages and sends to main loop
/// - main loop: coordinates input, output, and server communication
async fn run_client_loop(
    initial: Box<core::Session>,
    endpoint_methods: Vec<String>,
    cols: u16,
    rows: u16,
    initial_cell_width_px: u32,
    initial_cell_height_px: u32,
    initial_pixel_geometry_exact: bool,
    should_quit: Arc<AtomicBool>,
    mut config: ClientLoopConfig,
    _terminal_guard: &TerminalGuard,
) -> Result<(), ClientError> {
    let draw_host_cursor = should_draw_host_cursor(config.host_cursor);

    let mut state = ClientState {
        blit_encoder: render_ansi::BlitEncoder::new(),
        image_files: image_files::FileTransport::from_environment(),
        mouse_capture_active: config.mouse_capture_active,
        endpoint_mouse_capture_requested: false,
        endpoint_sgr_pixels_requested: false,
        shell_mouse_capture_preference: config.mouse_capture_active,
        pane_keyboard_report_all: false,
        keyboard_report_all_active: false,
        reported_size: (cols, rows),
        reported_cell_size: (initial_cell_width_px, initial_cell_height_px),
        sound_config: config.sound_config,
        kitty_graphics_enabled: config.kitty_graphics_enabled,
        pixel_geometry_enabled: config.pixel_geometry_enabled,
        pixel_geometry_exact: initial_pixel_geometry_exact,
        host_sgr_pixel_mouse: config.host_sgr_pixel_mouse,
        direct_graphics_response: Arc::new(Mutex::new(direct_graphics::ResponseMatcher::default())),
        retired_direct_graphics: None,
        disabled_native_graphics: Default::default(),
        pending_native_cleanup: Vec::new(),
        pending_surface_graphics: HashMap::new(),
        redraw_on_focus_gained: config.redraw_on_focus_gained,
        repaint_pending: false,
        draw_host_cursor,
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

    // Channel for events from the resize and server reader threads.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    // Keep Windows console draining independent of server-frame backpressure.
    let stdin_tx = event_tx.clone();

    let mut endpoint_commands = endpoint_commands::EndpointCommands::default();

    // Spawn the stdin reader thread.
    let will_query_host_terminal_theme = true;
    let host_theme_query_pending = Arc::new(AtomicU32::new(0));
    let stdin_host_theme_query_pending = host_theme_query_pending.clone();
    // Terminals behind ConPTY report no pixel size through the ioctl, so ask the
    // host terminal directly instead of falling back to an assumed cell size.
    let will_query_host_cell_size = host_cell_size_query_required(state.kitty_graphics_enabled);
    let stdin_quit = should_quit.clone();
    let stdin_mouse_capture_active = host_mouse_capture_active.clone();
    let stdin_sgr_pixels_active = host_sgr_pixels_active.clone();
    let stdin_escape_disambiguation_active = config.host_escape_disambiguation_active;
    let stdin_initial_host_input = std::mem::take(&mut config.initial_host_input);
    let stdin_direct_response = state.direct_graphics_response.clone();
    let stdin_direct_response_active = stdin_direct_response
        .lock()
        .map(|matcher| matcher.active_handle())
        .unwrap_or_default();
    std::thread::spawn(move || {
        input::stdin_reader_loop(
            stdin_tx,
            &stdin_quit,
            will_query_host_terminal_theme,
            stdin_host_theme_query_pending,
            will_query_host_cell_size,
            stdin_mouse_capture_active,
            stdin_sgr_pixels_active,
            stdin_escape_disambiguation_active,
            stdin_initial_host_input,
            stdin_direct_response,
            stdin_direct_response_active,
        );
    });

    if will_query_host_terminal_theme {
        query_host_terminal_theme();
        {
            query_host_terminal_appearance();
        }
    }

    if will_query_host_cell_size {
        query_host_cell_size();
    }

    // Spawn the resize poller thread.
    let resize_quit = should_quit.clone();
    let resize_tx = event_tx.clone();
    let resize_cell_size = reported_cell_size.clone();
    let pixel_geometry_enabled = state.pixel_geometry_enabled;
    let pixel_geometry_fallback = config.pixel_geometry_fallback;
    std::thread::spawn(move || {
        resize_poll_loop(
            resize_tx,
            cols,
            rows,
            initial_cell_width_px,
            initial_cell_height_px,
            initial_pixel_geometry_exact,
            pixel_geometry_enabled,
            pixel_geometry_fallback,
            &resize_cell_size,
            &resize_quit,
        );
    });

    let transport = core::start(initial, &event_tx).map_err(ClientError::ConnectionFailed)?;
    let mut connection = connection::CoreConnection::new(transport, 1);
    connection.send(&ClientMessage::ClientShellFocus { focused: true });

    // Main event loop.
    let mut client_timer = timer::ClientLoopTimer::new();
    while !should_quit.load(Ordering::Acquire) {
        {
            let shell = &mut state.shell;
            let cleanup = shell.take_pending_graphics_cleanup();
            state.present_graphics(&cleanup);
        }
        // Idle sessions wake once a second at most. The tick still reaps detached
        // children and expires graphics/health state; those deadlines are seconds
        // away, so a finer timer would only repaint frames that draw nothing.
        const HOUSEKEEPING_TICK: Duration = Duration::from_secs(1);
        let timer_delay = state
            .shell
            .timer_delay(std::time::Instant::now())
            .unwrap_or(HOUSEKEEPING_TICK);
        let timer_deadline = client_timer.deadline(std::time::Instant::now(), timer_delay);
        let event = tokio::select! {
            biased;
            _=tokio::time::sleep_until(timer_deadline.into())=>ClientLoopEvent::Timer,
            ev=event_rx.recv()=>ev.unwrap_or(ClientLoopEvent::Timer),
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
            ClientLoopEvent::DirectGraphicsResponse(response) => {
                let pending_key = state
                    .pending_surface_graphics
                    .keys()
                    .find(|(_, transfer_id, image_id)| {
                        *transfer_id == response.transfer_id && *image_id == response.image_id
                    })
                    .copied();
                let pending =
                    pending_key.and_then(|key| state.pending_surface_graphics.remove(&key));
                let Some(asset) = pending else {
                    // Raw/non-surface transfers retain their existing response handling.
                    continue;
                };
                let eligible = response.success
                    && connection.is_connected()
                    && state
                        .shell
                        .accepts_direct_graphics_asset(&asset, response.image_id);
                let size = state.reported_size;
                let prepared = eligible
                    .then(|| {
                        let shell = &mut state.shell;
                        let checkpoint = shell.direct_graphics_checkpoint();
                        if !shell.trust_direct_graphics_asset(&asset, response.image_id) {
                            shell.restore_direct_graphics_checkpoint(checkpoint);
                            return None;
                        }
                        match shell.compose(size.0, size.1) {
                            Some(frame) => Some((frame, checkpoint)),
                            None => {
                                shell.restore_direct_graphics_checkpoint(checkpoint);
                                None
                            }
                        }
                    })
                    .flatten();
                let accepted = if let Some((frame, checkpoint)) = prepared {
                    if state.try_present_frame(frame) {
                        true
                    } else {
                        state.shell.restore_direct_graphics_checkpoint(checkpoint);
                        false
                    }
                } else {
                    false
                };
                if !accepted {
                    // The upload may have reached the terminal even when its response, owner, or
                    // composed swap cannot be accepted. Never leave that unowned image resident.
                    // Restoring the checkpoint also restores old-bank stale-image bookkeeping.
                    state.queue_native_image_cleanup(response.image_id);
                }
                let message = ClientMessage::GraphicsTransmissionResult {
                    transfer_id: response.transfer_id,
                    image_id: response.image_id,
                    success: accepted,
                };
                connection.send(&message);
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
                // On Unix, palette changes may lack a color-scheme notification.
                // Re-query on redraw, including SIGWINCH without a resize.
                if will_query_host_terminal_theme {
                    host_theme_query_pending.fetch_add(1, Ordering::AcqRel);
                    query_host_terminal_theme();
                }
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
            ClientLoopEvent::ServerMessage {
                generation,
                message,
            } => {
                if !connection.accepts(generation) {
                    continue;
                }
                // Retirements belong to the exact live connection, even after deactivation.
                // These still precede the presentation-specific asset checks.
                if matches!(
                    message.as_ref(),
                    ServerMessage::GraphicsTransmissionRetired { .. }
                ) {
                    if let ServerMessage::GraphicsTransmissionRetired {
                        transfer_id,
                        image_id,
                    } = message.as_ref()
                    {
                        let owner_active = connection.is_connected();
                        state.receive_graphics_retirement(
                            generation,
                            *transfer_id,
                            *image_id,
                            owner_active,
                        );
                    }
                    continue;
                }
                let endpoint_active = connection.is_connected();
                match *message {
                    ServerMessage::ClientShellSnapshot(snapshot) => {
                        for id in endpoint_commands.observe_snapshot(generation, &snapshot.boot_id)
                        {
                            {
                                let shell = &mut state.shell;
                                shell.cancel_endpoint_request(&id);
                            }
                        }
                        install_client_shell_snapshot(&mut state, snapshot, &mut connection)?;
                    }
                    ServerMessage::PaneSurface(surface) => {
                        if !endpoint_active {
                            continue;
                        }
                        let composed = {
                            let shell = &mut state.shell;
                            shell.set_pane_surface(surface);
                            shell.compose(state.reported_size.0, state.reported_size.1)
                        };
                        if let Some(frame) = composed {
                            state.present_frame(frame);
                        }
                    }
                    ServerMessage::PaneSurfacePatch(patch) => {
                        let patch_started = crate::render_prof::timer();
                        let apply_started = crate::render_prof::timer();
                        let outcome = state.shell.apply_pane_surface_patch(patch);
                        crate::render_prof::duration_since(
                            "client_surface_patch.apply",
                            apply_started,
                        );
                        let compose_fallback = match outcome {
                            shell::ClientPaneSurfacePatchOutcome::Applied(Some(patch)) => {
                                match state.present_surface_patch(patch) {
                                    Ok(presented) => !presented,
                                    Err(error) => {
                                        warn!(%error, "failed to present retained pane surface patch");
                                        state.request_repaint();
                                        false
                                    }
                                }
                            }
                            shell::ClientPaneSurfacePatchOutcome::Applied(None) => true,
                            shell::ClientPaneSurfacePatchOutcome::Rejected => false,
                        };
                        if compose_fallback {
                            let composed = state
                                .shell
                                .compose(state.reported_size.0, state.reported_size.1);
                            if let Some(frame) = composed {
                                state.present_frame(frame);
                            }
                        }
                        crate::render_prof::duration_since(
                            "client_surface_patch.total",
                            patch_started,
                        );
                        crate::render_prof::flush_if_due();
                    }
                    ServerMessage::Terminal(frame) => {
                        if state.kitty_graphics_enabled
                            && contains_kitty_graphics_bytes(&frame.bytes)
                        {
                            record_received_kitty_graphics(&frame.bytes);
                        }
                        let mut stdout = io::stdout();
                        let _ = stdout.write_all(&frame.bytes);
                        let _ = stdout.flush();
                    }
                    ServerMessage::Graphics { bytes } => {
                        if state.kitty_graphics_enabled {
                            record_received_kitty_graphics(&bytes);
                            let mut stdout = io::stdout();
                            let _ = stdout.write_all(&bytes);
                            let _ = stdout.flush();
                        }
                    }
                    ServerMessage::TerminalBell { count } => {
                        if let Err(err) =
                            crate::terminal_effects::write_terminal_bells(&mut io::stdout(), count)
                        {
                            warn!(err = %err, "failed to emit terminal bell");
                        }
                    }
                    ServerMessage::GraphicsFile {
                        path,
                        expected_len,
                        image_id,
                        transfer_id,
                        leading,
                        control,
                        surface_asset,
                    } => {
                        let retirement =
                            state.match_retired_direct_graphics(generation, transfer_id, image_id);
                        if retirement == RetiredDirectGraphicsMatch::Exact {
                            continue;
                        }
                        let surface_asset_valid = surface_asset.as_ref().is_some_and(|asset| {
                            state.shell.accepts_direct_graphics_asset(asset, image_id)
                        });
                        let native = surface_asset.as_ref().is_some_and(|asset| {
                            matches!(
                                asset.source,
                                crate::protocol::SurfaceGraphicsSource::Terminal { .. }
                            )
                        });
                        let native_valid = !native
                            || surface_asset.as_ref().is_some_and(|asset| {
                                state.disabled_native_graphics != Some(generation)
                                    && connection.is_connected()
                                    && leading.is_empty()
                                    && asset.format == crate::protocol::SurfaceGraphicsFormat::Rgba
                                    && expected_len == asset.data_len
                                    && control
                                        == format!(
                                            "a=t,f=32,s={},v={},i={image_id},q=0",
                                            asset.image_width, asset.image_height
                                        )
                            });
                        let valid = retirement != RetiredDirectGraphicsMatch::Saturated
                            && state.kitty_graphics_enabled
                            && native_valid
                            && surface_asset_valid
                            && usize::try_from(expected_len).ok().is_some_and(|len| {
                                let path = std::path::Path::new(&path);
                                let valid_source =
                                    crate::pane_graphics_files::validate_direct_source(path, len)
                                        .is_ok()
                                        || (native
                                            && crate::pane_graphics_files::validate_native_source(
                                                path, len,
                                            )
                                            .is_ok());
                                valid_source
                                    && direct_graphics::valid_control(&control, image_id, len)
                            })
                            && state
                                .direct_graphics_response
                                .lock()
                                .is_ok_and(|mut matcher| matcher.arm(transfer_id, image_id));
                        let sent = if valid {
                            let mut command = Vec::new();
                            crate::kitty_graphics::encode_kitty_regular_file(
                                &mut command,
                                &leading,
                                &control,
                                &path,
                            );
                            let mut stdout = io::stdout();
                            let written = state
                                .flush_native_cleanup(&mut stdout)
                                .and_then(|()| stdout.write_all(&command))
                                .and_then(|()| stdout.flush())
                                .is_ok();
                            if written {
                                record_received_kitty_graphics(&command);
                            }
                            written
                        } else {
                            false
                        };
                        if sent {
                            if let Some(asset) = surface_asset {
                                state
                                    .pending_surface_graphics
                                    .insert((generation, transfer_id, image_id), asset);
                            }
                            if let Ok(mut matcher) = state.direct_graphics_response.lock() {
                                matcher.start(transfer_id);
                            }
                            let started = ClientMessage::GraphicsTransmissionStarted {
                                transfer_id,
                                image_id,
                            };
                            connection.send(&started);
                        } else {
                            state.pending_surface_graphics.remove(&(
                                generation,
                                transfer_id,
                                image_id,
                            ));
                            if let Ok(mut matcher) = state.direct_graphics_response.lock() {
                                if valid {
                                    matcher.retire(transfer_id);
                                } else {
                                    matcher.cancel(transfer_id);
                                }
                            }
                            let result = ClientMessage::GraphicsTransmissionResult {
                                transfer_id,
                                image_id,
                                success: false,
                            };
                            connection.send(&result);
                        }
                    }
                    ServerMessage::GraphicsTransmissionRetired { .. } => {
                        unreachable!("retirements are handled before presentation gating")
                    }
                    ServerMessage::ServerShutdown { reason } => {
                        return Err(ClientError::ServerShutdown { reason });
                    }
                    ServerMessage::Notify { .. } => {}
                    ServerMessage::SemanticNotification(event) => {
                        let (effects, frame) = {
                            let shell = &mut state.shell;
                            let (effects, repaint) =
                                shell.receive_notification(event, std::time::Instant::now());
                            let frame = repaint
                                .then(|| {
                                    shell.compose(state.reported_size.0, state.reported_size.1)
                                })
                                .flatten();
                            (effects, frame)
                        };
                        handle_shell_notification_effects(effects, &state.sound_config);
                        if let Some(frame) = frame {
                            state.present_frame(frame);
                        }
                    }
                    ServerMessage::ClientShellError { message } => {
                        let shell = &mut state.shell;
                        if shell.receive_endpoint_error(message) {
                            let frame = shell.compose(state.reported_size.0, state.reported_size.1);
                            if let Some(frame) = frame {
                                state.present_frame(frame);
                            }
                        }
                    }
                    ServerMessage::ClientShellEndpointResponse {
                        boot_id,
                        request_id,
                        result,
                    } => {
                        if request_id.starts_with("client-shell-surface:") {
                            continue;
                        }
                        let completed = endpoint_commands.receive_response(
                            generation,
                            &boot_id,
                            &request_id,
                            result,
                        );
                        let Some(completed) = completed else {
                            continue;
                        };
                        let (repaint, actions) = if completed.generation == generation {
                            state.shell.handle_endpoint_result(
                                &completed.boot_id,
                                &completed.request_id,
                                completed.result,
                            )
                        } else {
                            (
                                state.shell.cancel_endpoint_request(&completed.request_id),
                                Vec::new(),
                            )
                        };
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
                    ServerMessage::Clipboard { data } => {
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
                    ServerMessage::WindowTitle { title } => {
                        let _ = crate::terminal_effects::write_window_title(
                            &mut io::stdout(),
                            title.as_deref(),
                        );
                    }
                    ServerMessage::ReloadSoundConfig => apply_reload(
                        &mut state,
                        &mut connection,
                        &host_mouse_capture_active,
                        &host_sgr_pixels_active,
                    )?,
                    ServerMessage::MouseCapture {
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
                    ServerMessage::DirectTerminalKeyboardProtocol { .. } => {}
                    ServerMessage::ClientShellKeyboardReportAll { enabled } => {
                        state.pane_keyboard_report_all = enabled;
                        sync_client_shell_keyboard_report_all(&mut state)?;
                    }
                    ServerMessage::ClientShellEndpointMethods { methods } => {
                        install_client_shell_methods(&mut state, methods);
                    }
                }
            }
            ClientLoopEvent::ServerDisconnected { generation } => {
                if !connection.accepts(generation) {
                    continue;
                }
                connection.fail(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection was lost",
                ));
            }
            ClientLoopEvent::PaneDiagnosticsFinished { path, warning } => {
                let shell = &mut state.shell;
                if shell.diagnostic_notice(&path, warning.as_deref()) {
                    if let Some(frame) = shell.compose(state.reported_size.0, state.reported_size.1)
                    {
                        state.present_frame(frame);
                    }
                }
            }
            ClientLoopEvent::Timer => {
                client_timer.fired();
                if let Ok(mut matcher) = state.direct_graphics_response.lock() {
                    matcher.expire();
                }
                state
                    .detached_process_children
                    .retain_mut(|child| child.try_wait().ok().flatten().is_none());
                if let Some(failure) = connection.take_failure() {
                    if !failure_is_superseded(&connection, failure.generation) {
                        warn!(
                            endpoint = "local",
                            error = %failure.message,
                            "endpoint transport failed"
                        );
                        return Err(ClientError::ConnectionLost(io::Error::new(
                            failure.kind,
                            failure.message,
                        )));
                    }
                }
                {
                    let expired_endpoints = endpoint_commands
                        .expire(now)
                        .into_iter()
                        .filter(|expired| connection.accepts(expired.generation))
                        .collect::<Vec<_>>();
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
                    handle_shell_notification_effects(effects, &state.sound_config);
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

    // Clean exit (Ctrl+C). Send Detach before closing.
    let detach = ClientMessage::Detach;
    connection.send(&detach);
    let _ = io::stdout().flush();

    Ok(())
}

#[cfg(test)]
mod tests;
