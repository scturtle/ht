use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
