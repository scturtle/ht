//! Core runtime mounted into the existing client loop, not a second TUI.
mod copy;
mod effects;
mod endpoint;
pub(in crate::client) mod input;
mod metadata;
mod methods;
mod model;
mod organization;
mod organization_actions;
mod projection;
#[cfg(test)]
mod scrollback_tests;
mod session;
mod shared;
mod swap;
#[cfg(test)]
mod tests;
mod transport;
pub(in crate::client) mod view;
mod wake;
mod writer;
mod zoom;

use super::events::ClientLoopEvent;
use crate::protocol::{ClientMessage, ServerMessage};
#[cfg(test)]
pub(in crate::client) use endpoint::test_support;
pub(super) use endpoint::EndpointHandle;
#[cfg(test)]
use endpoint::InputBatch;
use endpoint::WorkerCommand;
pub(super) use session::Session;
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Options {
    pub binary: PathBuf,
    pub home: PathBuf,
    /// Socket this client drives: explicit `HC_SOCKET`, otherwise
    /// `$XDG_RUNTIME_DIR/herdr-core/<session>.sock`, as with `hc`.
    /// The client never keeps a socket of its own.
    pub socket: PathBuf,
    /// A private session owns its Core end to end and isolates its state dirs.
    pub isolate: bool,
    pub client_id: Option<String>,
}
fn presentation_identity(
    explicit: Option<String>,
    terminal: Option<String>,
) -> io::Result<Option<String>> {
    use sha2::Digest;
    if let Some(id) = explicit {
        if id.is_empty()
            || id.len() > 96
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HERDR_CORE_CLIENT_ID requires 1..96 ASCII letters, digits, '-' or '_'",
            ));
        }
        return Ok(Some(id));
    }
    Ok(terminal.map(|name| format!("tty-{:x}", sha2::Sha256::digest(name.as_bytes()))))
}
/// `hc`'s socket for a session: `$XDG_RUNTIME_DIR/herdr-core/<session>.sock`.
///
/// Falling back to the state directory keeps the client usable where no runtime
/// directory is exported, without giving it a socket of its own.
fn session_socket(session: &str) -> io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::config::state_dir()
                .join("core-client-v1")
                .join("run")
        });
    let dir = runtime.join("herdr-core");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{session}.sock")))
}

fn selected_socket(explicit: Option<PathBuf>, session: &str, isolate: bool) -> io::Result<PathBuf> {
    if let Some(socket) = explicit {
        if isolate {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HC_SOCKET cannot be combined with --core-session; unset HC_SOCKET to select a private session",
            ));
        }
        if socket.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HC_SOCKET must not be empty",
            ));
        }
        return Ok(socket);
    }
    session_socket(session)
}

impl Options {
    pub fn from_environment() -> io::Result<Self> {
        let binary = std::env::var_os("HERDR_CORE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("herdr-core"));
        // An explicit `--core-home` argument, or the default state directory. The
        // environment never moves it, so no enclosing process can retarget us.
        let home = crate::session::data_dir();
        let isolate = crate::session::core_session_is_private();
        let socket = selected_socket(
            std::env::var_os("HC_SOCKET").map(PathBuf::from),
            &crate::session::core_session(),
            isolate,
        )?;
        let explicit = match std::env::var("HERDR_CORE_CLIENT_ID") {
            Ok(id) => Some(id),
            Err(std::env::VarError::NotPresent) => None,
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidInput, e)),
        };
        let client_id =
            presentation_identity(explicit, crate::platform::client_terminal_identity())?;
        Ok(Self {
            binary,
            home,
            socket,
            isolate,
            client_id,
        })
    }
}

pub(super) fn prepare(
    options: Options,
    size: crate::protocol::ClientSurfaceSize,
    cells: (u32, u32),
    config: &crate::config::Config,
) -> io::Result<(Box<Session>, Vec<String>)> {
    let mut session = Session::connect(options, size, cells, config)?;
    // Explicit user startup, NOT connection/recovery: reuse the normal creation
    // action when the previous session was fully closed. Never adopt old programs.
    if session.model.records.is_empty() {
        session.organization_action(crate::actions::Action::WorkspaceCreate(
            crate::actions::WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: None,
                focus: true,
                label: None,
                env: Default::default(),
            },
        ))?;
        session.sync_views()?;
        session.save()?;
    }
    let endpoint_methods = methods::advertised(&session.core.hello);
    Ok((Box::new(session), endpoint_methods))
}

/// How long a lost Core connection is retried before the TUI gives up.
const RECONNECT_WINDOW: Duration = Duration::from_secs(30);
/// A planned restart brings Core back in well under a second. A runtime that
/// stays absent this long was stopped on purpose, not left recovering.
const RECONNECT_ABSENT_WINDOW: Duration = Duration::from_secs(5);
/// Bounded exponential backoff between recovery probes.
fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_millis(100u64 << attempt.min(4))
}

fn should_wait_for_work(worked: bool, session: &Session) -> bool {
    !worked && !session.dirty && !session.snapshot_dirty
}

fn recoverable(error: &io::Error) -> bool {
    !matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
    )
}

/// Discard input queued for the connection that just died. It was addressed to
/// the previous Core: replaying it could act on stale geometry or modes.
fn drain_input(receiver: &mpsc::Receiver<WorkerCommand>, queued: &AtomicUsize) -> usize {
    let mut dropped = 0;
    while let Ok(command) = receiver.try_recv() {
        match command {
            WorkerCommand::Input(batch) => {
                if let Ok(mut batch) = batch.lock() {
                    let batch = std::mem::take(&mut *batch);
                    queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                    dropped += batch.messages.len();
                }
            }
            WorkerCommand::Flush(done) => {
                // A flush barrier for the dead connection is satisfied, not lost.
                let _ = done.send(());
            }
        }
    }
    dropped
}

/// Deliver adapter output through the existing client event loop.
fn emit(
    events: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
    message: ServerMessage,
) -> io::Result<()> {
    events
        .blocking_send(ClientLoopEvent::ServerMessage {
            generation: 1,
            message: Box::new(message),
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client stopped"))
}

/// Drive one connection until a client-initiated end (Ok) or a failure (Err).
fn run_connection(
    session: &mut Session,
    receiver: &mpsc::Receiver<WorkerCommand>,
    queued: &AtomicUsize,
    stopped: &AtomicBool,
    events: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
) -> io::Result<()> {
    let wake = session.core.worker_wake();
    wake.bind_current();
    while !stopped.load(Ordering::Acquire) {
        for message in session.publish()? {
            emit(events, message)?;
        }
        if session.model.records.is_empty() {
            // Last-pane closure leaves the TUI, but never stops Core.
            emit(
                events,
                ServerMessage::ServerShutdown {
                    reason: Some("detached".into()),
                },
            )?;
            return Ok(());
        }
        let mut worked = false;
        match receiver.try_recv() {
            Ok(WorkerCommand::Input(batch)) => {
                worked = true;
                let batch = std::mem::take(
                    &mut *batch
                        .lock()
                        .map_err(|_| io::Error::other("Core input batch poisoned"))?,
                );
                queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                for message in batch.messages {
                    if matches!(message, ClientMessage::Detach) {
                        session.save()?;
                        emit(
                            events,
                            ServerMessage::ServerShutdown {
                                reason: Some("detached".into()),
                            },
                        )?;
                        return Ok(());
                    }
                    for reply in session.message(message)? {
                        emit(events, reply)?;
                    }
                    if session.model.records.is_empty() {
                        break;
                    }
                }
            }
            Ok(WorkerCommand::Flush(done)) => {
                worked = true;
                let _ = done.send(());
            }
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        // Bound output work so busy programs cannot starve UI input/publication.
        for _ in 0..128 {
            let Some(frame) = session.core.poll()? else {
                break;
            };
            worked = true;
            for message in session.consume(frame)? {
                emit(events, message)?;
            }
        }
        if should_wait_for_work(worked, session) {
            // Reader, writer, commands and stop retain a wake token.
            wake.wait(session.wait_deadline());
        }
    }
    Ok(())
}

/// Recover only a running Core, discarding input addressed to the old connection.
fn recover_connection(
    session: &mut Session,
    receiver: &mpsc::Receiver<WorkerCommand>,
    queued: &AtomicUsize,
    stopped: &AtomicBool,
    wake: &wake::Wake,
    error: io::Error,
) -> io::Result<()> {
    let cause = error.to_string();
    let dropped = drain_input(receiver, queued);
    let started = Instant::now();
    let socket = session.socket_path();
    let mut absent_since: Option<Instant> = None;
    let mut attempt = 0;
    loop {
        if stopped.load(Ordering::Acquire) || started.elapsed() >= RECONNECT_WINDOW {
            return Err(error);
        }
        // A planned restart recreates its socket quickly. Do not undo a stop.
        if socket.exists() {
            absent_since = None;
        } else {
            let since = *absent_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= RECONNECT_ABSENT_WINDOW {
                return Err(error);
            }
        }
        let until = Instant::now() + reconnect_delay(attempt);
        while !stopped.load(Ordering::Acquire) && Instant::now() < until {
            drain_input(receiver, queued);
            wake.wait(Some(until));
        }
        if stopped.load(Ordering::Acquire) {
            return Err(error);
        }
        attempt += 1;
        // resume uses connect_existing: a stopped runtime is never started.
        match session.resume() {
            Ok(()) => {
                tracing::info!(
                    error = %cause,
                    dropped,
                    "Core connection recovered; delivered images are not re-sent"
                );
                drain_input(receiver, queued);
                return Ok(());
            }
            Err(resume) if !recoverable(&resume) => return Err(resume),
            Err(resume) => {
                tracing::debug!(attempt, error = %resume, "Core recovery probe failed");
            }
        }
    }
}

pub(super) fn start(
    mut session: Box<Session>,
    events: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
) -> io::Result<EndpointHandle> {
    let (endpoint, receiver) = EndpointHandle::channel(session.core.worker_wake());
    let wake = endpoint.wake.clone();
    let queued = endpoint.queued.clone();
    let stopped = endpoint.stopped.clone();
    let error = endpoint.error.clone();
    let events = events.clone();
    std::thread::Builder::new()
        .name("core-endpoint".into())
        .spawn(move || {
            wake.bind_current();
            let result = loop {
                match run_connection(&mut session, &receiver, &queued, &stopped, &events) {
                    Ok(()) => break Ok(()),
                    Err(lost) if !recoverable(&lost) => break Err(lost),
                    Err(lost) => {
                        if let Err(failed) = recover_connection(
                            &mut session,
                            &receiver,
                            &queued,
                            &stopped,
                            &wake,
                            lost,
                        ) {
                            break Err(failed);
                        }
                        // Re-gate the shell before publishing the replacement projection.
                        if let Err(failed) = emit(
                            &events,
                            ServerMessage::ClientShellEndpointMethods {
                                methods: methods::advertised(&session.core.hello),
                            },
                        ) {
                            break Err(failed);
                        }
                    }
                }
            };
            if let Err(failed) = result {
                tracing::warn!(error = %failed, "Core endpoint failed");
                if let Ok(mut slot) = error.lock() {
                    *slot = Some(failed);
                }
            }
            stopped.store(true, Ordering::Release);
            let _ = events.blocking_send(ClientLoopEvent::ServerDisconnected { generation: 1 });
            // Dropping only the relay leaves Core's holders and programs alive.
        })?;
    Ok(endpoint)
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn retry_backoff_is_bounded_and_monotonic() {
        let delays = (0..8).map(reconnect_delay).collect::<Vec<_>>();
        assert_eq!(delays[0], Duration::from_millis(100));
        assert!(
            delays.windows(2).all(|pair| pair[0] <= pair[1]),
            "{delays:?} must not shrink"
        );
        // A retry loop must not turn into a long sleep that ignores the window.
        assert!(delays.iter().all(|d| *d <= Duration::from_millis(1600)));
        assert_eq!(delays[4], delays[7]);
    }

    #[test]
    fn an_outage_is_retried_but_a_missing_capability_is_not() {
        assert!(recoverable(&io::Error::from(io::ErrorKind::BrokenPipe)));
        assert!(recoverable(&io::Error::from(io::ErrorKind::UnexpectedEof)));
        assert!(recoverable(&io::Error::from(io::ErrorKind::TimedOut)));
        // Retrying a Core that cannot refuse to start would risk resurrecting it.
        assert!(!recoverable(&io::Error::new(
            io::ErrorKind::Unsupported,
            "Core lacks existing_connect"
        )));
        assert!(!recoverable(&io::Error::new(
            io::ErrorKind::InvalidInput,
            "bad identity"
        )));
    }

    #[test]
    fn queued_input_is_discarded_and_flush_barriers_are_satisfied() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let queued = AtomicUsize::new(0);
        let mut expected = 0;
        for _ in 0..3 {
            let bytes = 10;
            queued.fetch_add(bytes, Ordering::AcqRel);
            expected += bytes;
            sender
                .send(WorkerCommand::Input(Arc::new(Mutex::new(InputBatch {
                    messages: vec![ClientMessage::Detach, ClientMessage::Detach],
                    bytes,
                }))))
                .expect("queued batch");
        }
        let (done, wait) = mpsc::channel();
        sender
            .send(WorkerCommand::Flush(done))
            .expect("queued flush");

        let dropped = drain_input(&receiver, &queued);
        assert_eq!(dropped, 6, "every queued message must be accounted for");
        assert_eq!(queued.load(Ordering::Acquire), 0, "accounting must reset");
        // A flush issued for the dead connection was satisfied by discarding it.
        assert!(wait.recv_timeout(Duration::from_millis(50)).is_ok());
        assert_eq!(expected, 30);
        assert!(receiver.try_recv().is_err(), "the queue must be empty");
    }
}
