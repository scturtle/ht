//! A single Core transport and its connection-generation fence, not a federation registry.
use super::ClientEndpointId;
use crate::protocol::ClientMessage;
use std::{io, time::Instant};

pub(crate) trait EndpointTransport: Send {
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
pub(crate) struct EndpointConnection {
    transport: Box<dyn EndpointTransport>,
    pub(crate) generation: u64,
}
pub(crate) struct EndpointTransportFailure {
    pub(crate) endpoint_id: ClientEndpointId,
    pub(crate) generation: u64,
    pub(crate) kind: io::ErrorKind,
    pub(crate) message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndpointSendOutcome {
    Sent,
    NotSent,
}
pub(crate) struct EndpointRegistry {
    connection: Option<EndpointConnection>,
    failure: Option<EndpointTransportFailure>,
}
impl EndpointRegistry {
    pub(crate) fn new(transport: impl EndpointTransport + 'static, generation: u64) -> Self {
        Self {
            connection: Some(EndpointConnection {
                transport: Box::new(transport),
                generation,
            }),
            failure: None,
        }
    }
    pub(crate) fn active_id(&self) -> &ClientEndpointId {
        &ClientEndpointId::Local
    }
    pub(crate) fn connection(&self, _id: &ClientEndpointId) -> Option<&EndpointConnection> {
        self.connection.as_ref()
    }
    pub(crate) fn active_surface_available(&self) -> bool {
        self.connection.is_some()
    }
    pub(crate) fn accepts(&self, _id: &ClientEndpointId, generation: u64) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|c| c.generation == generation)
    }
    /// The diagnostic lane remains usable when presentation/input is unavailable.
    pub(crate) fn diagnostic_source(
        &mut self,
        pane: &str,
        client: &mut serde_json::Value,
    ) -> Option<crate::client::diagnostics::DiagnosticSource> {
        client["endpoint_registry"] = serde_json::json!({"input_enabled":self.active_surface_available(),
            "surface_available":self.active_surface_available(),"endpoint":"local",
            "generation":self.connection.as_ref().map(|c|c.generation)});
        self.connection.as_mut()?.transport.diagnostic_source(pane)
    }
    pub(crate) fn send(&mut self, message: &ClientMessage) -> EndpointSendOutcome {
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "Core connection unavailable")
            })
            .and_then(|c| c.transport.send(message));
        match result {
            Ok(()) => EndpointSendOutcome::Sent,
            Err(error) => {
                self.fail(&ClientEndpointId::Local, error);
                EndpointSendOutcome::NotSent
            }
        }
    }
    pub(crate) fn send_to(
        &mut self,
        _id: &ClientEndpointId,
        message: &ClientMessage,
    ) -> EndpointSendOutcome {
        self.send(message)
    }
    pub(crate) fn fail(&mut self, _id: &ClientEndpointId, error: io::Error) {
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        let error = connection.transport.take_error().unwrap_or(error);
        connection.transport.disconnect();
        self.failure = Some(EndpointTransportFailure {
            endpoint_id: ClientEndpointId::Local,
            generation: connection.generation,
            kind: error.kind(),
            message: error.to_string(),
        });
    }
    pub(crate) fn take_failures(&mut self) -> Vec<EndpointTransportFailure> {
        let error = self
            .connection
            .as_mut()
            .and_then(|c| c.transport.take_error());
        if let Some(error) = error {
            self.fail(&ClientEndpointId::Local, error);
        }
        self.failure.take().into_iter().collect()
    }
}
impl Drop for EndpointRegistry {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            let _ = connection.transport.send(&ClientMessage::Detach);
            let _ = connection
                .transport
                .flush(Instant::now() + std::time::Duration::from_millis(250));
            connection.transport.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Capture(Arc<Mutex<Vec<&'static str>>>);
    impl EndpointTransport for Capture {
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
        let registry = EndpointRegistry::new(Capture(Default::default()), 7);
        assert!(registry.accepts(&ClientEndpointId::Local, 7));
        assert!(!registry.accepts(&ClientEndpointId::Local, 6));
    }
    #[test]
    fn detach_flushes_before_only_its_own_relay_is_disconnected() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        drop(EndpointRegistry::new(Capture(calls.clone()), 1));
        assert_eq!(*calls.lock().unwrap(), ["send", "flush", "disconnect"]);
    }
    #[test]
    fn a_failed_connection_is_revoked_once_and_cannot_receive_input() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = EndpointRegistry::new(Capture(calls.clone()), 1);
        registry.fail(
            &ClientEndpointId::Local,
            io::Error::new(io::ErrorKind::BrokenPipe, "lost"),
        );
        assert!(!registry.accepts(&ClientEndpointId::Local, 1));
        assert_eq!(
            registry.send(&ClientMessage::Detach),
            EndpointSendOutcome::NotSent
        );
        assert_eq!(registry.take_failures().len(), 1);
        assert!(registry.take_failures().is_empty());
        drop(registry);
        assert_eq!(*calls.lock().unwrap(), ["disconnect"]);
    }
}
