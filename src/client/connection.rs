//! The one Core connection of this client. Other ht clients are independent peers.
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

pub(crate) trait CoreTransport: Send {
    fn send(&mut self, message: &ClientMessage) -> io::Result<()>;
    fn disconnect(&mut self) {}
    fn flush(&mut self, _deadline: Instant) -> io::Result<()> {
        Ok(())
    }
    fn take_error(&mut self) -> Option<io::Error> {
        None
    }
    fn diagnostic_source(
        &mut self,
        _pane: &str,
    ) -> Option<crate::client::diagnostics::DiagnosticSource> {
        None
    }
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
/// This client's sole Core transport and its connection-generation fence.
pub(crate) struct CoreConnection {
    transport: Option<Box<dyn CoreTransport>>,
    generation: u64,
    failure: Option<ConnectionFailure>,
}
impl CoreConnection {
    pub(crate) fn new(transport: impl CoreTransport + 'static, generation: u64) -> Self {
        Self {
            transport: Some(Box::new(transport)),
            generation,
            failure: None,
        }
    }
    pub(crate) fn generation(&self) -> Option<u64> {
        self.transport.as_ref().map(|_| self.generation)
    }
    pub(crate) fn is_connected(&self) -> bool {
        self.transport.is_some()
    }
    pub(crate) fn accepts(&self, generation: u64) -> bool {
        self.generation() == Some(generation)
    }
    /// The diagnostic lane remains usable when presentation/input is unavailable.
    pub(crate) fn diagnostic_source(
        &mut self,
        pane: &str,
        client: &mut serde_json::Value,
    ) -> Option<crate::client::diagnostics::DiagnosticSource> {
        // Preserve the existing dump field name for diagnostic readers.
        client["endpoint_registry"] = serde_json::json!({"input_enabled":self.is_connected(),
            "surface_available":self.is_connected(),"endpoint":"local",
            "generation":self.generation()});
        self.transport.as_mut()?.diagnostic_source(pane)
    }
    /// Failed sends revoke the transport and leave one failure for the lifecycle
    /// loop. Callers must not bypass that transition or retry uncertain delivery.
    pub(crate) fn send(&mut self, message: &ClientMessage) -> SendOutcome {
        let result = self
            .transport
            .as_mut()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Core connection unavailable")
            })
            .and_then(|transport| transport.send(message));
        match result {
            Ok(()) => SendOutcome::Sent,
            Err(error) => {
                self.fail(error);
                SendOutcome::NotSent
            }
        }
    }
    pub(crate) fn fail(&mut self, error: io::Error) {
        let Some(mut transport) = self.transport.take() else {
            return;
        };
        let error = transport.take_error().unwrap_or(error);
        transport.disconnect();
        self.failure = Some(ConnectionFailure {
            generation: self.generation,
            kind: error.kind(),
            message: error.to_string(),
        });
    }
    pub(crate) fn take_failure(&mut self) -> Option<ConnectionFailure> {
        let error = self
            .transport
            .as_mut()
            .and_then(|transport| transport.take_error());
        if let Some(error) = error {
            self.fail(error);
        }
        self.failure.take()
    }
}
impl Drop for CoreConnection {
    fn drop(&mut self) {
        if let Some(transport) = self.transport.as_mut() {
            let _ = transport.send(&ClientMessage::Detach);
            let _ = transport.flush(Instant::now() + std::time::Duration::from_millis(250));
            transport.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Capture(Arc<Mutex<Vec<&'static str>>>);
    impl CoreTransport for Capture {
        fn send(&mut self, _: &ClientMessage) -> io::Result<()> {
            self.0.lock().unwrap().push("send");
            Ok(())
        }
        fn flush(&mut self, _: Instant) -> io::Result<()> {
            self.0.lock().unwrap().push("flush");
            Ok(())
        }
        fn disconnect(&mut self) {
            self.0.lock().unwrap().push("disconnect");
        }
    }
    #[test]
    fn stale_connection_generations_remain_fenced() {
        let connection = CoreConnection::new(Capture(Default::default()), 7);
        assert_eq!(connection.generation(), Some(7));
        assert!(connection.accepts(7));
        assert!(!connection.accepts(6));
    }
    #[test]
    fn detach_flushes_before_only_its_own_relay_is_disconnected() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        drop(CoreConnection::new(Capture(calls.clone()), 1));
        assert_eq!(*calls.lock().unwrap(), ["send", "flush", "disconnect"]);
    }
    #[test]
    fn a_failed_connection_is_revoked_once_and_cannot_receive_input() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut connection = CoreConnection::new(Capture(calls.clone()), 1);
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
        assert_eq!(*calls.lock().unwrap(), ["disconnect"]);
    }

    #[test]
    fn failed_send_preserves_the_writer_error_for_one_lifecycle_transition() {
        struct FailedWriter(Arc<Mutex<Vec<&'static str>>>);
        impl CoreTransport for FailedWriter {
            fn send(&mut self, _: &ClientMessage) -> io::Result<()> {
                self.0.lock().unwrap().push("send");
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "queue closed"))
            }
            fn take_error(&mut self) -> Option<io::Error> {
                Some(io::Error::new(io::ErrorKind::TimedOut, "writer deadline"))
            }
            fn disconnect(&mut self) {
                self.0.lock().unwrap().push("disconnect");
            }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut connection = CoreConnection::new(FailedWriter(calls.clone()), 7);
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
        assert_eq!(*calls.lock().unwrap(), ["send", "disconnect"]);
    }

    #[test]
    fn asynchronous_transport_failure_is_revoked_and_consumed_once() {
        struct AsyncFailure(Option<io::Error>);
        impl CoreTransport for AsyncFailure {
            fn send(&mut self, _: &ClientMessage) -> io::Result<()> {
                Ok(())
            }
            fn take_error(&mut self) -> Option<io::Error> {
                self.0.take()
            }
        }
        let mut connection = CoreConnection::new(
            AsyncFailure(Some(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "worker stopped",
            ))),
            9,
        );
        let failure = connection.take_failure().unwrap();
        assert_eq!(failure.generation, 9);
        assert_eq!(failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(failure.message, "worker stopped");
        assert!(!connection.accepts(9));
        assert!(connection.take_failure().is_none());
    }
}
