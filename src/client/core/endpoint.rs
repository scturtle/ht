//! Typed, bounded input queue between the UI and the Core adapter worker.
use super::wake;
use crate::protocol::ClientMessage;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Instant,
};

#[derive(Default)]
pub(super) struct InputBatch {
    pub(super) messages: Vec<ClientMessage>,
    pub(super) bytes: usize,
}
pub(super) enum WorkerCommand {
    Input(Arc<Mutex<InputBatch>>),
    Flush(mpsc::Sender<()>),
}
/// UI-side handle to the adapter worker; sending here only queues typed messages.
pub(in crate::client) struct EndpointHandle {
    sender: mpsc::SyncSender<WorkerCommand>,
    pub(super) wake: Arc<wake::Wake>,
    pending: Option<Arc<Mutex<InputBatch>>>,
    pub(super) queued: Arc<AtomicUsize>,
    pub(super) stopped: Arc<AtomicBool>,
    pub(super) error: Arc<Mutex<Option<io::Error>>>,
}
impl EndpointHandle {
    pub(super) fn channel(wake: Arc<wake::Wake>) -> (Self, mpsc::Receiver<WorkerCommand>) {
        let (sender, receiver) = mpsc::sync_channel(64);
        (
            Self {
                sender,
                wake,
                pending: None,
                queued: Arc::new(AtomicUsize::new(0)),
                stopped: Arc::new(AtomicBool::new(false)),
                error: Arc::new(Mutex::new(None)),
            },
            receiver,
        )
    }

    /// `false` means a cleanly stopped worker no longer accepts input. The UI
    /// learns about the detach separately; late input is not a transport failure.
    pub(in crate::client) fn send(&mut self, message: &ClientMessage) -> io::Result<bool> {
        if self.stopped.load(Ordering::Acquire) {
            if self.error.lock().is_ok_and(|error| error.is_none()) {
                return Ok(false);
            }
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Core endpoint stopped",
            ));
        }
        // In-process only: these are typed values, never an encoded envelope.
        let bytes = crate::protocol::queued_message_cost(message)
            .saturating_add(std::mem::size_of::<ClientMessage>());
        if bytes > 1024 * 1024 {
            return Err(io::Error::other("Core input exceeds 1 MiB"));
        }
        let message = message.clone();
        self.queued
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
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
                return Ok(true);
            }
        }
        let batch = Arc::new(Mutex::new(InputBatch {
            messages: vec![message],
            bytes,
        }));
        if self
            .sender
            .try_send(WorkerCommand::Input(batch.clone()))
            .is_err()
        {
            self.queued.fetch_sub(bytes, Ordering::AcqRel);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Core endpoint input queue full or closed",
            ));
        }
        self.pending = Some(batch);
        self.wake.notify();
        Ok(true)
    }
    pub(in crate::client) fn disconnect(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify();
    }
    pub(in crate::client) fn flush(&mut self, deadline: Instant) -> io::Result<()> {
        self.pending = None; // Preserve the flush barrier: never append behind it.
        let (done, wait) = mpsc::channel();
        self.sender
            .try_send(WorkerCommand::Flush(done))
            .map_err(|_| io::Error::other("Core flush queue closed"))?;
        self.wake.notify();
        wait.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(io::Error::other)
    }
    pub(in crate::client) fn take_error(&mut self) -> Option<io::Error> {
        self.error.lock().ok()?.take()
    }
}
impl Drop for EndpointHandle {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Tests use the real queue and shared failure state, not a second endpoint implementation.
#[cfg(test)]
pub(in crate::client) mod test_support {
    use super::*;

    pub(in crate::client) struct EndpointProbe {
        receiver: mpsc::Receiver<WorkerCommand>,
        queued: Arc<AtomicUsize>,
        stopped: Arc<AtomicBool>,
        error: Arc<Mutex<Option<io::Error>>>,
    }

    pub(in crate::client) fn endpoint() -> (EndpointHandle, EndpointProbe) {
        let (endpoint, receiver) = EndpointHandle::channel(Arc::default());
        let probe = EndpointProbe {
            receiver,
            queued: endpoint.queued.clone(),
            stopped: endpoint.stopped.clone(),
            error: endpoint.error.clone(),
        };
        (endpoint, probe)
    }

    impl EndpointProbe {
        pub(in crate::client) fn drain(&self) -> Vec<ClientMessage> {
            let mut messages = Vec::new();
            while let Ok(command) = self.receiver.try_recv() {
                match command {
                    WorkerCommand::Input(batch) => {
                        let batch = std::mem::take(&mut *batch.lock().unwrap());
                        self.queued.fetch_sub(batch.bytes, Ordering::AcqRel);
                        messages.extend(batch.messages);
                    }
                    WorkerCommand::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
            messages
        }

        pub(in crate::client) fn fail(&self, error: io::Error) {
            *self.error.lock().unwrap() = Some(error);
            self.stopped.store(true, Ordering::Release);
        }

        pub(in crate::client) fn is_stopped(&self) -> bool {
            self.stopped.load(Ordering::Acquire)
        }

        pub(in crate::client) fn acknowledge_detach(&self) {
            let command = self
                .receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            let WorkerCommand::Input(batch) = command else {
                panic!("detach must precede the flush barrier");
            };
            let batch = std::mem::take(&mut *batch.lock().unwrap());
            self.queued.fetch_sub(batch.bytes, Ordering::AcqRel);
            assert_eq!(batch.messages, vec![ClientMessage::Detach]);
            let command = self
                .receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            let WorkerCommand::Flush(done) = command else {
                panic!("flush must follow detach");
            };
            assert!(
                !self.is_stopped(),
                "disconnect must follow flush acknowledgement"
            );
            done.send(()).unwrap();
        }
    }
}
