//! Explicit, private incident captures. Never run on render/PTY polling paths.
use super::{core, events::ClientLoopEvent};
use serde_json::{json, Value};
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) struct DiagnosticSource {
    pub(super) options: core::Options,
    pub(super) local: Result<mpsc::Receiver<Value>, String>,
    pub(super) events: tokio::sync::mpsc::Sender<ClientLoopEvent>,
}

static CAPTURING: AtomicBool = AtomicBool::new(false);
struct CaptureGuard;
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::Release);
    }
}

/// Local data is saved even if the endpoint queue, relay or Core is unresponsive.
pub(super) fn start(
    pane: String,
    client: Value,
    source: Option<DiagnosticSource>,
) -> io::Result<PathBuf> {
    CAPTURING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "a pane diagnostic capture is already running",
            )
        })?;
    let guard = CaptureGuard;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let path = PathBuf::from("/var/tmp").join(format!(
        "ht-pane-dump-{}-{}",
        std::process::id(),
        stamp.as_nanos()
    ));
    crate::platform::create_remote_private_dir(&path)?;
    let target = path.clone();
    std::thread::Builder::new()
        .name("pane-diagnostics".into())
        .spawn(move || capture(guard, pane, client, source, target, stamp))?;
    Ok(path)
}

fn capture(
    guard: CaptureGuard,
    pane: String,
    mut client: Value,
    source: Option<DiagnosticSource>,
    target: PathBuf,
    stamp: Duration,
) {
    let _guard = guard;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(4);
    let mut errors = Vec::new();
    if pane.is_empty() {
        errors.push(
            "client snapshot had no focused pane; only connection/client state was requested"
                .into(),
        );
    }
    let save = |name: &str, value: &Value| -> io::Result<()> {
        std::fs::write(target.join(name), serde_json::to_vec_pretty(value)?)
    };
    client["process"] = crate::platform::diagnostic_process_snapshot();
    if let Err(error) = save("client.json", &client) {
        errors.push(error.to_string());
    }
    let events = source.as_ref().map(|s| s.events.clone());
    let source_identity = source.as_ref().map(
        |s| json!({"binary":s.options.binary,"socket":s.options.socket,"home":s.options.home}),
    );
    if let Some(source) = source {
        match core::diagnostic_probe(&source.options, &pane, deadline) {
            Ok(value) => {
                if let Some(results) = value.as_object() {
                    for (method, result) in results {
                        if let Some(error) = result["error"].as_str() {
                            errors.push(format!("{method}: {error}"));
                        }
                    }
                }
                if let Some(text) = value["terminal.read"]["text"].as_str() {
                    if let Err(error) = std::fs::write(target.join("core-screen.txt"), text) {
                        errors.push(error.to_string());
                    }
                }
                if let Err(error) = save("core.json", &value) {
                    errors.push(error.to_string());
                }
            }
            Err(error) => {
                errors.push(format!("Core read-only probe: {error}"));
                let _ = save("core.json", &json!({"error":error.to_string()}));
            }
        }
        let local = source.local.and_then(|rx| {
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| format!("adapter snapshot unavailable: {e}"))
        });
        let value = match local {
            Ok(value) => {
                if let Some(error) = value["error"].as_str() {
                    errors.push(error.into());
                } else if let Some(error) = value
                    .pointer("/view/terminal/error")
                    .and_then(Value::as_str)
                {
                    errors.push(format!("pane parser snapshot: {error}"));
                } else if !pane.is_empty() && value.get("view").is_some_and(Value::is_null) {
                    errors.push("adapter had no view for the requested pane".into());
                }
                value
            }
            Err(error) => {
                errors.push(error.clone());
                json!({"error":error})
            }
        };
        if let Err(error) = save("adapter.json", &value) {
            errors.push(error.to_string());
        }
    } else {
        errors.push("no active Core transport; only client state was available".into());
    }
    let manifest = json!({
        "schema":1, "pid":std::process::id(), "version":env!("CARGO_PKG_VERSION"),
        "executable":std::env::current_exe().ok(), "pane":pane, "core_source":source_identity,
        "captured_at_unix_ms":stamp.as_millis(), "duration_ms":started.elapsed().as_millis(),
        "errors":errors, "status":if errors.is_empty() { "complete" } else { "partial" },
        "consistency":"client, adapter and Core are sampled independently; compare revisions and RPC timings",
        "privacy":"Contains pane screen text and program facts. Keep private. No input, focus, resize, attach or program lifecycle operations were sent."
    });
    if let Err(error) = save("report.json", &manifest) {
        errors.push(error.to_string());
    }
    let warning = (!errors.is_empty()).then(|| errors.join("; "));
    tracing::info!(path=%target.display(), partial=warning.is_some(), "pane diagnostic capture saved");
    if let Some(events) = events {
        let _ = events.try_send(ClientLoopEvent::PaneDiagnosticsFinished {
            path: target,
            warning,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn wait_for_capture(path: &std::path::Path) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if !CAPTURING.load(Ordering::Acquire) {
                if let Ok(bytes) = std::fs::read(path.join("report.json")) {
                    if let Ok(value) = serde_json::from_slice(&bytes) {
                        return value;
                    }
                }
            }
            assert!(Instant::now() < deadline, "capture did not complete");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn local_diagnostics_are_saved_without_a_transport() {
        let _lock = LOCK.lock().unwrap();
        let path = start("p1".into(), json!({"mode":"Terminal"}), None).unwrap();
        let report = wait_for_capture(&path);
        assert_eq!(report["pane"], "p1");
        assert_eq!(report["errors"].as_array().unwrap().len(), 1);
        let client: Value =
            serde_json::from_slice(&std::fs::read(path.join("client.json")).unwrap()).unwrap();
        assert_eq!(client["mode"], "Terminal");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn an_unresponsive_adapter_produces_a_bounded_partial_dump() {
        let _lock = LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!(
            "ht-dump-source-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (_hold, receive) = mpsc::channel();
        let (events, mut notices) = tokio::sync::mpsc::channel(2);
        let source = DiagnosticSource {
            options: core::Options {
                binary: home.join("not-a-Core-binary"),
                socket: home.join("not-a-socket"),
                home: home.clone(),
                isolate: true,
                client_id: None,
            },
            local: Ok(receive),
            events,
        };
        let started = Instant::now();
        let path = start("p1".into(), json!({"frozen":true}), Some(source)).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "UI must not await the adapter"
        );
        assert_eq!(
            start("p1".into(), json!({}), None).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let report = wait_for_capture(&path);
        assert!(report["errors"].as_array().unwrap().len() >= 2);
        assert!(path.join("client.json").exists());
        assert!(path.join("adapter.json").exists());
        assert!(matches!(
            notices.try_recv().unwrap(),
            ClientLoopEvent::PaneDiagnosticsFinished {
                warning: Some(_),
                ..
            }
        ));
        std::fs::remove_dir_all(path).unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }
}
