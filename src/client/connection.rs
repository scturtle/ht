//! The UI's endpoint lifecycle. Core relay I/O and recovery live in `client::core`.
use super::core::EndpointHandle;
use crate::protocol::ClientMessage;
use std::{io, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientEndpointStatus {
    /// No projection has arrived yet.
    Connecting,
    Online,
    #[cfg(test)]
    Reconnecting,
}

pub(crate) struct ConnectionFailure {
    pub(crate) generation: u64,
    pub(crate) kind: io::ErrorKind,
    pub(crate) message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    Sent,
    NotSent,
}
/// Owns the UI endpoint, revokes it on failure, and fences events by generation.
pub(crate) struct EndpointConnection {
    endpoint: Option<EndpointHandle>,
    generation: u64,
    failure: Option<ConnectionFailure>,
}
impl EndpointConnection {
    pub(super) fn new(endpoint: EndpointHandle, generation: u64) -> Self {
        Self {
            endpoint: Some(endpoint),
            generation,
            failure: None,
        }
    }
    pub(crate) fn generation(&self) -> Option<u64> {
        self.endpoint.as_ref().map(|_| self.generation)
    }
    pub(crate) fn is_connected(&self) -> bool {
        self.endpoint.is_some()
    }
    pub(crate) fn accepts(&self, generation: u64) -> bool {
        self.generation() == Some(generation)
    }
    /// Failed sends revoke the endpoint and leave one failure for the lifecycle
    /// loop. Callers must not bypass that transition or retry uncertain delivery.
    pub(crate) fn send(&mut self, message: &ClientMessage) -> SendOutcome {
        let result = self
            .endpoint
            .as_mut()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Core connection unavailable")
            })
            .and_then(|endpoint| endpoint.send(message));
        match result {
            Ok(true) => SendOutcome::Sent,
            Ok(false) => SendOutcome::NotSent,
            Err(error) => {
                self.fail(error);
                SendOutcome::NotSent
            }
        }
    }
    pub(crate) fn fail(&mut self, error: io::Error) {
        let Some(mut endpoint) = self.endpoint.take() else {
            return;
        };
        let error = endpoint.take_error().unwrap_or(error);
        endpoint.disconnect();
        self.failure = Some(ConnectionFailure {
            generation: self.generation,
            kind: error.kind(),
            message: error.to_string(),
        });
    }
    pub(crate) fn take_failure(&mut self) -> Option<ConnectionFailure> {
        let error = self
            .endpoint
            .as_mut()
            .and_then(|endpoint| endpoint.take_error());
        if let Some(error) = error {
            self.fail(error);
        }
        self.failure.take()
    }
}
impl Drop for EndpointConnection {
    fn drop(&mut self) {
        if let Some(endpoint) = self.endpoint.as_mut() {
            let _ = endpoint.send(&ClientMessage::Detach);
            let _ = endpoint.flush(Instant::now() + std::time::Duration::from_millis(250));
            endpoint.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::core::test_support::endpoint;

    #[test]
    fn clean_worker_stop_does_not_turn_late_mouse_input_into_failure() {
        let (mut endpoint, probe) = endpoint();
        // Same shared state as a worker that published graceful ServerShutdown
        // and stopped before the UI dequeues it.
        endpoint.disconnect();
        let mut connection = EndpointConnection::new(endpoint, 1);
        for bytes in [b"\x1b[<35;66;20M".as_slice(), b"\x1b[<64;66;20M", b"\x1b[A"] {
            assert_eq!(
                connection.send(&ClientMessage::Input {
                    data: bytes.to_vec()
                }),
                SendOutcome::NotSent
            );
        }
        assert!(probe.drain().is_empty());
        assert!(connection.take_failure().is_none());
        assert!(
            connection.accepts(1),
            "queued shutdown must still be accepted"
        );
    }

    #[test]
    fn failed_worker_stop_still_revokes_endpoint_and_preserves_real_error() {
        let (endpoint, probe) = endpoint();
        probe.fail(io::Error::new(io::ErrorKind::TimedOut, "writer deadline"));
        let mut connection = EndpointConnection::new(endpoint, 1);
        assert_eq!(
            connection.send(&ClientMessage::Input {
                data: b"\x1b[<64;66;20M".to_vec()
            }),
            SendOutcome::NotSent
        );
        let failure = connection.take_failure().unwrap();
        assert_eq!(failure.kind, io::ErrorKind::TimedOut);
        assert_eq!(failure.message, "writer deadline");
        assert!(!connection.accepts(1));
        assert!(probe.drain().is_empty());
    }

    #[test]
    fn stale_connection_generations_remain_fenced() {
        let (handle, probe) = endpoint();
        let connection = EndpointConnection::new(handle, 7);
        assert_eq!(connection.generation(), Some(7));
        assert!(connection.accepts(7));
        assert!(!connection.accepts(6));
        // Closing the worker queue also makes teardown non-blocking.
        drop(probe);
    }

    #[test]
    fn detach_flushes_before_only_its_own_relay_is_disconnected() {
        let (handle, probe) = endpoint();
        let connection = EndpointConnection::new(handle, 1);
        let worker = std::thread::spawn(move || {
            probe.acknowledge_detach();
            probe
        });
        drop(connection);
        assert!(worker.join().unwrap().is_stopped());
    }

    #[test]
    fn a_failed_connection_is_revoked_once_and_cannot_receive_input() {
        let (handle, probe) = endpoint();
        let mut connection = EndpointConnection::new(handle, 1);
        connection.fail(io::Error::new(io::ErrorKind::BrokenPipe, "lost"));
        assert!(!connection.accepts(1));
        assert_eq!(connection.generation(), None);
        assert_eq!(
            connection.send(&ClientMessage::Detach),
            SendOutcome::NotSent
        );
        assert!(connection.take_failure().is_some_and(
            |failure| failure.generation == 1 && failure.kind == io::ErrorKind::BrokenPipe
        ));
        assert!(connection.take_failure().is_none());
        drop(connection);
        assert!(probe.is_stopped());
        assert!(
            probe.drain().is_empty(),
            "revoked endpoints receive no input"
        );
    }

    #[test]
    fn failed_send_preserves_the_writer_error_for_one_lifecycle_transition() {
        let (handle, probe) = endpoint();
        let mut connection = EndpointConnection::new(handle, 7);
        probe.fail(io::Error::new(io::ErrorKind::TimedOut, "writer deadline"));
        assert_eq!(
            connection.send(&ClientMessage::Detach),
            SendOutcome::NotSent
        );
        assert!(!connection.is_connected());
        assert_eq!(connection.generation(), None);
        // Repeated sends cannot retry delivery or overwrite the original error.
        assert_eq!(
            connection.send(&ClientMessage::Detach),
            SendOutcome::NotSent
        );
        let failure = connection.take_failure().unwrap();
        assert_eq!(failure.generation, 7);
        assert_eq!(failure.kind, io::ErrorKind::TimedOut);
        assert_eq!(failure.message, "writer deadline");
        assert!(connection.take_failure().is_none());
        drop(connection);
        assert!(probe.drain().is_empty());
    }

    #[test]
    fn asynchronous_worker_failure_is_revoked_and_consumed_once() {
        let (handle, probe) = endpoint();
        let mut connection = EndpointConnection::new(handle, 9);
        probe.fail(io::Error::new(io::ErrorKind::BrokenPipe, "worker stopped"));
        let failure = connection.take_failure().unwrap();
        assert_eq!(failure.generation, 9);
        assert_eq!(failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(failure.message, "worker stopped");
        assert!(!connection.accepts(9));
        assert!(connection.take_failure().is_none());
    }
}
