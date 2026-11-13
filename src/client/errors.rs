use std::io;

/// Endpoint failures are client-local. No error path starts a legacy runtime.
#[derive(Debug)]
pub enum ClientError {
    ConnectionFailed(io::Error),
    ServerShutdown { reason: Option<String> },
    ConnectionLost(io::Error),
}
impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(err)=>write!(f,"failed to connect to Core: {err}\nCheck HERDR_CORE_BIN, HC_SOCKET, --core-session and --core-home; there is no legacy fallback."),
            Self::ServerShutdown{reason}=>{
                if reason.as_deref()==Some("detached") {write!(f,"detached from Core\nRun `ht` to reattach with the same Core selection and --core-home")}
                else {write!(f,"Core server shut down")?;if let Some(reason)=reason {write!(f,": {reason}")?;}Ok(())}
            },
            Self::ConnectionLost(err)=>write!(f,"lost connection to Core: {err}\nIf Core survived, its panes may still be running.\nRun `ht` to reattach with the same Core selection and --core-home"),
        }
    }
}
impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConnectionFailed(e) | Self::ConnectionLost(e) => Some(e),
            _ => None,
        }
    }
}
