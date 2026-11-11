//! Opt-in Core runtime mounted into the existing client loop, not a second TUI.
mod copy;
mod effects;
pub(in crate::client) mod input;
mod metadata;
mod methods;
mod model;
mod organization;
mod organization_actions;
mod projection;
mod session;
mod shared;
mod swap;
#[cfg(test)]
mod tests;
mod transport;
pub(in crate::client) mod view;
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

#[derive(Clone)]
pub(super) struct Options {
    pub binary: PathBuf,
    pub home: PathBuf,
    /// Socket this client drives: `$XDG_RUNTIME_DIR/herdr-core/<session>.sock`,
    /// the same convention `hc` uses, so both tools reach one Core. The client
    /// never keeps a socket of its own.
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

impl Options {
    pub fn from_environment() -> io::Result<Self> {
        let binary = std::env::var_os("HERDR_CORE_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("herdr-core"));
        // An explicit `--core-home` argument, or the default state directory. The
        // environment never moves it, so no enclosing process can retarget us.
        let home = crate::session::data_dir();
        let isolate = crate::session::core_session_is_private();
        let socket = session_socket(&crate::session::core_session())?;
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
) -> io::Result<(super::InitialTransport, super::handshake::HandshakeResult)> {
    let mut session = Session::connect(options, size, cells, config)?;
    // Explicit user startup, NOT connection/recovery: reuse the normal creation
    // action when the previous session was fully closed. Never adopt old programs.
    if session.model.records.is_empty() {
        session.organization_action(crate::api::schema::Method::WorkspaceCreate(
            crate::api::schema::WorkspaceCreateParams {
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
    Ok((
        super::InitialTransport::Core(Box::new(session)),
        super::handshake::HandshakeResult {
            encoding: crate::protocol::RenderEncoding::SemanticFrame,
            endpoint_methods: Some(endpoint_methods),
            endpoint_capabilities: Some(Vec::new()),
        },
    ))
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
}
struct CoreEndpoint {
    sender: mpsc::SyncSender<Command>,
    pending: Option<Arc<Mutex<Batch>>>,
    queued: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    error: Arc<Mutex<Option<io::Error>>>,
}
impl EndpointTransport for CoreEndpoint {
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
        Ok(())
    }
    fn disconnect(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
    fn flush(&mut self, deadline: Instant) -> io::Result<()> {
        self.pending = None; // Preserve the flush barrier: never append behind it.
        let (done, wait) = mpsc::channel();
        self.sender
            .try_send(Command::Flush(done))
            .map_err(|_| io::Error::other("Core flush queue closed"))?;
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
                if let Ok(batch) = batch.lock() {
                    queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                    dropped += batch.messages.len();
                }
            }
            Command::Flush(done) => {
                // A flush barrier for the dead connection is satisfied, not lost.
                let _ = done.send(());
            }
        }
    }
    dropped
}

pub(super) fn start(
    mut session: Box<Session>,
    events: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
) -> io::Result<impl EndpointTransport + 'static> {
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
                        match receiver.recv_timeout(Duration::from_millis(8)) {
                            Ok(Command::Batch(batch)) => {
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
                                let _ = done.send(());
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                return Ok(SessionEnd::Finished);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        for _ in 0..128 {
                            let Some(frame) = session.core.poll()? else {
                                break;
                            };
                            for message in session.consume(frame)? {
                                emit(message)?;
                            }
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
                            std::thread::sleep(reconnect_delay(attempt));
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
            let _ = events.blocking_send(ClientLoopEvent::ServerDisconnected {
                endpoint_id: ClientEndpointId::Local,
                generation: 1,
            });
            // Dropping only the relay leaves Core's holders and programs alive.
        })?;
    Ok(CoreEndpoint {
        sender,
        pending: None,
        queued,
        stopped,
        error,
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
