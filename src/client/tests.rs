use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn resize_signal_reports_even_when_polled_size_is_unchanged() {
    let size = (120, 40, 8, 16, true);
    assert!(resize_report_required(true, size, size));
    assert!(!resize_report_required(false, size, size));
    assert!(resize_report_required(false, (120, 41, 8, 16, true), size));
    assert!(resize_report_required(false, (120, 40, 9, 18, true), size));
    assert!(resize_report_required(false, (120, 40, 8, 16, false), size));
}

#[test]
fn unavailable_terminal_grid_is_not_fabricated() {
    let reported_cell_size = AtomicU64::new(0);
    let err = current_terminal_geometry_with(false, false, &reported_cell_size, None, None, || {
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

    super::terminal_geometry::resize_poll_loop_with(
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
            [super::ClientLoopEvent::Resize(80, 24, 8, 16, true)]
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

    super::terminal_geometry::resize_poll_loop_with(
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
            [super::ClientLoopEvent::TerminalUnavailable(err)]
                if err.kind() == io::ErrorKind::NotConnected
        ),
        "expected exactly one terminal-unavailable event"
    );
}

#[test]
fn missing_pixel_geometry_keeps_a_valid_terminal_grid() {
    let reported_cell_size = AtomicU64::new(0);
    let geometry = current_terminal_geometry_with(
        true,
        true,
        &reported_cell_size,
        Some((9, 18)),
        None,
        || Ok((80, 24)),
    )
    .expect("grid geometry remains valid without pixel dimensions");

    assert_eq!(geometry, (80, 24, 9, 18, false));
}

fn restore_env_var(key: &str, value: Option<OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        restore_env_var(self.key, self.previous.clone());
    }
}

#[test]
fn graphics_bytes_are_written_inside_synchronized_blit_with_saved_cursor() {
    let mut output = Vec::new();
    write_encoded_frame_with_graphics(
        &mut output,
        b"\x1b[?2026htext\x1b[?2026lcursor",
        b"graphics",
    )
    .unwrap();

    assert_eq!(
        output,
        b"\x1b[?2026htext\x1b7graphics\x1b8\x1b[?2026lcursor"
    );
}

#[test]
fn kitty_graphics_image_id_parser_tracks_herdr_ids_only() {
    let ids = kitty_graphics_image_ids(
        b"text\x1b_Ga=t,t=d,f=32,s=1,v=1,i=10023,q=2;AAAA\x1b\\\x1b_Ga=p,i=10023,p=7;\x1b\\",
    );
    assert_eq!(ids, vec![10023, 10023]);
}

#[test]
fn kitty_graphics_cleanup_deletes_tracked_images_not_all_images() {
    record_received_kitty_graphics(b"\x1b_Ga=t,i=123,q=2;AAAA\x1b\\");
    let mut output = Vec::new();
    clear_received_kitty_graphics(&mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("a=d,d=I,i=123"));
    assert!(!text.contains("d=A"));
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
fn reported_cell_size_is_taken_from_host_cell_size_events() {
    let events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b[?997;1n");
    assert_eq!(
        super::terminal_geometry::reported_cell_size_from_events(&events),
        None
    );

    let events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b[6;21;10t\x1b[6;18;9t");
    assert_eq!(
        super::terminal_geometry::reported_cell_size_from_events(&events),
        Some((9, 18))
    );
}

#[test]
fn client_error_display_connection_failed() {
    let err = ClientError::ConnectionFailed(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "connection refused",
    ));
    let msg = err.to_string();
    assert!(
        msg.contains("failed to connect to Core"),
        "should mention connection failure: {msg}"
    );
    assert!(
        msg.contains("HERDR_CORE_BIN") && !msg.contains("herdr server"),
        "must describe Core configuration without suggesting a removed server: {msg}"
    );
}

#[test]
fn client_error_display_detached_ignores_retired_session_environment() {
    let _guard = env_lock().lock().unwrap();
    let _session_env = EnvVarGuard::set(crate::session::SESSION_ENV_VAR, "work");
    let err = ClientError::ServerShutdown {
        reason: Some("detached".into()),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("Run `ht` to reattach") && !msg.contains("session attach"),
        "retired session environment must not suggest a removed command: {msg}"
    );
}

#[test]
fn client_error_display_remote_connection_lost_has_reattach_hint() {
    let _guard = env_lock().lock().unwrap();
    let err = ClientError::ConnectionLost(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"));
    let msg = err.to_string();
    assert!(
        msg.contains("lost connection to Core"),
        "should mention remote connection loss: {msg}"
    );
    assert!(
        msg.contains("panes may still be running"),
        "should explain possible persistence: {msg}"
    );
    assert!(
        msg.contains("Run `ht` to reattach") && !msg.contains("--remote"),
        "must show the supported Core reattach command: {msg}"
    );
}

#[test]
fn toast_notify_from_server_is_emitted_even_when_attach_config_was_off() {
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::Toast,
        "pi finished",
        Some("workspace 1"),
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
        |_, _| Ok(false),
    );

    assert_eq!(
        emitted,
        Some(("pi finished".to_string(), Some("workspace 1".to_string())))
    );
}

#[test]
fn system_toast_notify_from_server_uses_system_notifier() {
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::SystemToast,
        "pi finished",
        Some("workspace 1"),
        |_, _| Ok(false),
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
    );

    assert_eq!(
        emitted,
        Some(("pi finished".to_string(), Some("workspace 1".to_string())))
    );
}

#[test]
fn system_toast_notify_preserves_colon_in_title() {
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::SystemToast,
        "build: failed",
        Some("api workspace"),
        |_, _| Ok(false),
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
    );

    assert_eq!(
        emitted,
        Some((
            "build: failed".to_string(),
            Some("api workspace".to_string())
        ))
    );
}

#[test]
fn ioctl_cell_size_accepts_fractional_terminal_geometry() {
    assert_eq!(ioctl_cell_size(80, 24, 800, 480), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 805, 480), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 800, 485), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 0, 485), None);
}

struct SilentTransport;

impl connection::CoreTransport for SilentTransport {
    fn send(&mut self, _: &crate::protocol::ClientMessage) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn only_a_failure_from_the_live_generation_ends_the_client_loop() {
    let live = connection::CoreConnection::new(SilentTransport, 7);
    assert!(!failure_is_superseded(&live, 7));
    assert!(failure_is_superseded(&live, 6));

    let mut revoked = connection::CoreConnection::new(SilentTransport, 7);
    revoked.fail(io::Error::new(io::ErrorKind::BrokenPipe, "lost"));
    assert!(
        !failure_is_superseded(&revoked, 7),
        "a revoked connection reports its own failure as fatal"
    );
    assert!(revoked.take_failure().is_some());
    assert!(
        revoked.take_failure().is_none(),
        "a failure is consumed once"
    );
}
