use super::*;

pub(super) trait ClientMessageSink {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()>;
}

impl ClientMessageSink for endpoint::EndpointRegistry {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()> {
        // The lifecycle loop consumes failures for every endpoint, including Local. A send
        // failure must not bypass that transition or tear down unrelated connections.
        self.send(message);
        Ok(())
    }
}

pub(super) fn write_to_server(
    stream: &mut impl ClientMessageSink,
    msg: &ClientMessage,
) -> io::Result<()> {
    stream.send_client_message(msg)
}
