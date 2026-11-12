//! Core runtime mounted into the existing client loop, not a second TUI.
mod copy;
mod effects;
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

use super::{
    endpoint::{ClientEndpointId, EndpointTransport},
    events::ClientLoopEvent,
};
use crate::protocol::{ClientMessage, ServerMessage};
pub(super) use session::Session;
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
pub(super) use transport::diagnostic_probe;

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

struct QueuedMessage {
    message: ClientMessage,
}
#[derive(Default)]
struct Batch {
    messages: Vec<QueuedMessage>,
    bytes: usize,
}
enum Command {
    Batch(Arc<Mutex<Batch>>),
    Flush(mpsc::Sender<()>),
    Diagnostics {
        pane: String,
        deadline: Instant,
        done: mpsc::Sender<serde_json::Value>,
    },
}
struct CoreEndpoint {
    sender: mpsc::SyncSender<Command>,
    wake: Arc<wake::Wake>,
    pending: Option<Arc<Mutex<Batch>>>,
    queued: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    error: Arc<Mutex<Option<io::Error>>>,
    options: Option<Options>,
    events: tokio::sync::mpsc::Sender<ClientLoopEvent>,
}
impl EndpointTransport for CoreEndpoint {
    fn diagnostic_source(&mut self, pane: &str) -> Option<super::diagnostics::DiagnosticSource> {
        let options = self.options.clone()?;
        self.pending = None; // Keep later input behind the capture barrier.
        let (done, receive) = mpsc::channel();
        let local = self
            .sender
            .try_send(Command::Diagnostics {
                pane: pane.into(),
                deadline: Instant::now() + Duration::from_secs(4),
                done,
            })
            .map(|()| {
                self.wake.notify();
                receive
            })
            .map_err(|error| error.to_string());
        Some(super::diagnostics::DiagnosticSource {
            options,
            local,
            events: self.events.clone(),
        })
    }

    fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Core endpoint stopped",
            ));
        }
        // In-process only: these are typed values, never an encoded envelope.
        let bytes = crate::protocol::queued_message_cost(message)
            .saturating_add(std::mem::size_of::<QueuedMessage>());
        if bytes > 1024 * 1024 {
            return Err(io::Error::other("Core input exceeds 1 MiB"));
        }
        let message = QueuedMessage {
            message: message.clone(),
        };
        self.queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes).filter(|n| *n <= 4 * 1024 * 1024)
            })
            .map_err(|_| io::Error::other("Core input queue exceeds 4 MiB"))?;
        if let Some(pending) = &self.pending {
            let mut batch = pending
                .lock()
                .map_err(|_| io::Error::other("Core input batch poisoned"))?;
            if !batch.messages.is_empty() && batch.bytes.saturating_add(bytes) <= 64 * 1024 {
                batch.messages.push(message);
                batch.bytes += bytes;
                drop(batch);
                self.wake.notify();
                return Ok(());
            }
        }
        let batch = Arc::new(Mutex::new(Batch {
            messages: vec![message],
            bytes,
        }));
        if self.sender.try_send(Command::Batch(batch.clone())).is_err() {
            self.queued.fetch_sub(bytes, Ordering::AcqRel);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Core endpoint input queue full or closed",
            ));
        }
        self.pending = Some(batch);
        self.wake.notify();
        Ok(())
    }
    fn disconnect(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify();
    }
    fn flush(&mut self, deadline: Instant) -> io::Result<()> {
        self.pending = None; // Preserve the flush barrier: never append behind it.
        let (done, wait) = mpsc::channel();
        self.sender
            .try_send(Command::Flush(done))
            .map_err(|_| io::Error::other("Core flush queue closed"))?;
        self.wake.notify();
        wait.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(io::Error::other)
    }
    fn take_error(&mut self) -> Option<io::Error> {
        self.error.lock().ok()?.take()
    }
}
impl Drop for CoreEndpoint {
    fn drop(&mut self) {
        self.disconnect();
    }
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

/// Why the session loop ended. Only a finished session leaves the TUI.
enum SessionEnd {
    /// Client-initiated end: detach, fully closed workbench, or client exit.
    Finished,
    /// The connection failed and the adapter may attempt a recovery probe.
    Lost(io::Error),
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
fn drain_input(receiver: &mpsc::Receiver<Command>, queued: &AtomicUsize) -> usize {
    let mut dropped = 0;
    while let Ok(command) = receiver.try_recv() {
        match command {
            Command::Batch(batch) => {
                if let Ok(mut batch) = batch.lock() {
                    let batch = std::mem::take(&mut *batch);
                    queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                    dropped += batch.messages.len();
                }
            }
            Command::Flush(done) => {
                // A flush barrier for the dead connection is satisfied, not lost.
                let _ = done.send(());
            }
            Command::Diagnostics { done, .. } => {
                let _ = done
                    .send(serde_json::json!({"error":"connection lost before adapter capture"}));
            }
        }
    }
    dropped
}

pub(super) fn start(
    mut session: Box<Session>,
    events: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
) -> io::Result<impl EndpointTransport + 'static> {
    let options = session.diagnostic_options();
    let wake = session.core.worker_wake();
    let worker_wake = wake.clone();
    let diagnostic_events = events.clone();
    let (sender, receiver) = mpsc::sync_channel(64);
    let queued = Arc::new(AtomicUsize::new(0));
    let worker_queued = queued.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(None));
    let worker_stop = stopped.clone();
    let worker_error = error.clone();
    let events = events.clone();
    std::thread::Builder::new()
        .name("core-endpoint".into())
        .spawn(move || {
            worker_wake.bind_current();
            let emit = |message: ServerMessage| {
                events
                    .blocking_send(ClientLoopEvent::ServerMessage {
                        endpoint_id: ClientEndpointId::Local,
                        generation: 1,
                        message: Box::new(message),
                    })
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client stopped"))
            };
            // One connection attempt. A transport failure is recoverable while a
            // client-initiated end is final.
            let run = |session: &mut Box<Session>| -> SessionEnd {
                let connection_wake=session.core.worker_wake();
                connection_wake.bind_current();
                let step = (|| -> io::Result<SessionEnd> {
                    while !worker_stop.load(Ordering::Acquire) {
                        for message in session.publish()? {
                            emit(message)?;
                        }
                        if session.model.records.is_empty() {
                            // Match original last-pane closure: leave the TUI, not a
                            // blank workbench. Dropping this relay does NOT stop Core.
                            emit(ServerMessage::ServerShutdown {
                                reason: Some("detached".into()),
                            })?;
                            return Ok(SessionEnd::Finished);
                        }
                        let mut worked=false;
                        match receiver.try_recv() {
                            Ok(Command::Batch(batch)) => {
                                worked=true;
                                let batch =
                                    std::mem::take(&mut *batch.lock().map_err(|_| {
                                        io::Error::other("Core input batch poisoned")
                                    })?);
                                worker_queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                                for queued in batch.messages {
                                    let message = queued.message;
                                    if matches!(message, ClientMessage::Detach) {
                                        session.save()?;
                                        emit(ServerMessage::ServerShutdown {
                                            reason: Some("detached".into()),
                                        })?;
                                        return Ok(SessionEnd::Finished);
                                    }
                                    for reply in session.message(message)? {
                                        emit(reply)?;
                                    }
                                    if session.model.records.is_empty() {
                                        break;
                                    }
                                }
                            }
                            Ok(Command::Flush(done)) => {
                                worked=true;
                                let _ = done.send(());
                            }
                            Ok(Command::Diagnostics { pane, deadline, done }) => {
                                worked=true;
                                let value = if Instant::now() < deadline {
                                    session.diagnostic_snapshot(&pane)
                                } else {
                                    serde_json::json!({"error":"capture expired before the adapter became available"})
                                };
                                let _ = done.send(value);
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                return Ok(SessionEnd::Finished);
                            }
                            Err(mpsc::TryRecvError::Empty) => {}
                        }
                        for _ in 0..128 {
                            let Some(frame) = session.core.poll()? else {
                                break;
                            };
                            worked=true;
                            for message in session.consume(frame)? {
                                emit(message)?;
                            }
                        }
                        if should_wait_for_work(worked,session) {
                            // Reader, writer completion, commands and stop all retain a
                            // wake token. A stalled asynchronous credit keeps its deadline.
                            connection_wake.wait(session.wait_deadline());
                        }
                    }
                    Ok(SessionEnd::Finished)
                })();
                match step {
                    Ok(end) => end,
                    Err(error) => SessionEnd::Lost(error),
                }
            };
            let result = loop {
                match run(&mut session) {
                    SessionEnd::Finished => break Ok(()),
                    SessionEnd::Lost(error) if !recoverable(&error) => break Err(error),
                    SessionEnd::Lost(error) => {
                        let cause = error.to_string();
                        let dropped = drain_input(&receiver, &worker_queued);
                        let started = Instant::now();
                        let socket = session.socket_path();
                        let mut absent_since: Option<Instant> = None;
                        let mut attempt = 0;
                        let recovered = loop {
                            if worker_stop.load(Ordering::Acquire) {
                                break Err(error);
                            }
                            if started.elapsed() >= RECONNECT_WINDOW {
                                break Err(error);
                            }
                            // A restart recreates its socket in well under a
                            // second. One that stays missing was stopped on
                            // purpose, and a probe must not undo that.
                            if socket.exists() {
                                absent_since = None;
                            } else {
                                let since = *absent_since.get_or_insert_with(Instant::now);
                                if since.elapsed() >= RECONNECT_ABSENT_WINDOW {
                                    break Err(error);
                                }
                            }
                            let until=Instant::now()+reconnect_delay(attempt);
                            while !worker_stop.load(Ordering::Acquire) && Instant::now()<until {
                                drain_input(&receiver,&worker_queued);
                                worker_wake.wait(Some(until));
                            }
                            if worker_stop.load(Ordering::Acquire) {break Err(error);}
                            attempt += 1;
                            // A recovery probe reaches only a running Core, so a
                            // stop the user asked for is never undone.
                            match session.resume() {
                                Ok(()) => break Ok(()),
                                // A Core that cannot refuse to start is not a
                                // retryable outage: never probe it again.
                                Err(resume) if !recoverable(&resume) => break Err(resume),
                                Err(resume) => {
                                    tracing::debug!(
                                        attempt,
                                        error = %resume,
                                        "Core recovery probe failed"
                                    );
                                }
                            }
                        };
                        match recovered {
                            Ok(()) => {
                                tracing::info!(
                                    error = %cause,
                                    dropped,
                                    "Core connection recovered; delivered images are not re-sent"
                                );
                                drain_input(&receiver, &worker_queued);
                                // A replacement Core may implement a different method set.
                                // Authoritative, ordered and typed: the shell re-gates its
                                // menus and confirmations before the new projection arrives.
                                if let Err(stopped) = emit(
                                    ServerMessage::ClientShellEndpointMethods {
                                        methods: methods::advertised(&session.core.hello),
                                    },
                                ) {
                                    break Err(stopped);
                                }
                                continue;
                            }
                            Err(error) => break Err(error),
                        }
                    }
                }
            };
            if let Err(e) = result {
                tracing::warn!(error=%e,"Core endpoint failed");
                if let Ok(mut slot) = worker_error.lock() {
                    *slot = Some(e);
                }
            }
            worker_stop.store(true, Ordering::Release);
            let _ = events.blocking_send(ClientLoopEvent::ServerDisconnected { generation: 1 });
            // Dropping only the relay leaves Core's holders and programs alive.
        })?;
    Ok(CoreEndpoint {
        sender,
        wake,
        pending: None,
        queued,
        stopped,
        error,
        options,
        events: diagnostic_events,
    })
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

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
                .send(Command::Batch(Arc::new(Mutex::new(Batch {
                    messages: vec![
                        QueuedMessage {
                            message: ClientMessage::Detach,
                        },
                        QueuedMessage {
                            message: ClientMessage::Detach,
                        },
                    ],
                    bytes,
                }))))
                .expect("queued batch");
        }
        let (done, wait) = mpsc::channel();
        sender.send(Command::Flush(done)).expect("queued flush");

        let dropped = drain_input(&receiver, &queued);
        assert_eq!(dropped, 6, "every queued message must be accounted for");
        assert_eq!(queued.load(Ordering::Acquire), 0, "accounting must reset");
        // A flush issued for the dead connection was satisfied by discarding it.
        assert!(wait.recv_timeout(Duration::from_millis(50)).is_ok());
        assert_eq!(expected, 30);
        assert!(receiver.try_recv().is_err(), "the queue must be empty");
    }
}
