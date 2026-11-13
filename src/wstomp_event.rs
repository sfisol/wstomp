use async_stomp::{FromServer, Message, ToServer};
use awc::{
    error::{WsClientError, WsProtocolError},
    ws::CloseReason,
};
use tokio::sync::mpsc::error::SendError;

#[derive(Debug)]
#[non_exhaustive]
pub enum WStompConnectError {
    WsClientError(WsClientError),
    ConnectMessageFailed(Box<SendError<Message<ToServer>>>),
    ReconnectionLimit,
}

/// Custom enum combine events in WebSocket and STOMP
#[derive(Debug)]
#[non_exhaustive]
pub enum WStompEvent {
    /// Regular message from STOMP protocol
    Message(Message<FromServer>),
    /// WebSocket or STOMP error combined
    Error(WStompError),
}

impl From<WStompError> for WStompEvent {
    fn from(err: WStompError) -> Self {
        Self::Error(err)
    }
}

/// Custom error type to combine WebSocket and STOMP errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum WStompError {
    /// Error during receiving websocket frames (from awc)
    WsReceive(WsProtocolError),
    /// Error during sending websocket frames (from awc)
    WsSend(WsProtocolError),
    /// Error while decoding (receiving) STOMP message (from async-stomp), or a
    /// frame that grows past 16 MiB before it ends
    StompDecoding(anyhow::Error),
    /// Error while encoding (sending) STOMP message (from async-stomp)
    StompEncoding(anyhow::Error),
    /// Websocket closed connection (with reason)
    WebsocketClosed(Option<CloseReason>),
    /// Websocket connection ended without a Close frame (abnormal closure,
    /// code 1006), e.g. the server or something in between dropped it
    WebsocketDropped,
    /// Can't send ping, probably network problems
    PingFailed(anyhow::Error),
    /// Nothing at all, not even a pong, has arrived for two ping intervals
    PingTimeout,
}

impl WStompError {
    /// Whether the connection is gone after this error. Every error ends it
    /// except [`Self::StompEncoding`], which loses only the frame the app tried
    /// to send.
    pub fn is_fatal(&self) -> bool {
        !matches!(self, Self::StompEncoding(_))
    }
}

impl std::fmt::Display for WStompConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WsClientError(WsClientError::InvalidResponseStatus(status)) => write!(
                f,
                "WebSocket connection error: server rejected upgrade with HTTP {}",
                status
            ),
            Self::WsClientError(err) => write!(f, "WebSocket connection error: {}", err),
            Self::ConnectMessageFailed(msg) => write!(f, "WebSocket receive error: {}", msg),
            Self::ReconnectionLimit => write!(f, "Reconnection retry limit reached"),
        }
    }
}

impl std::error::Error for WStompConnectError {}

impl std::fmt::Display for WStompError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WsReceive(err) => write!(f, "WebSocket receive error: {}", err),
            Self::WsSend(err) => write!(f, "WebSocket send error: {}", err),
            Self::StompDecoding(err) => write!(f, "STOMP decoding error: {}", err),
            Self::StompEncoding(err) => write!(f, "STOMP encoding error: {}", err),
            Self::WebsocketClosed(reason) => write!(
                f,
                "Websocket closed {}",
                reason
                    .as_ref()
                    .map(|r| r.description.clone().unwrap_or_default())
                    .unwrap_or_default()
            ),
            Self::WebsocketDropped => write!(f, "Websocket dropped without a Close frame"),
            Self::PingFailed(err) => write!(f, "Websocket ping failed: {err}"),
            Self::PingTimeout => write!(f, "Websocket ping timeout"),
        }
    }
}

impl std::error::Error for WStompError {}

#[cfg(test)]
mod tests {
    use super::WStompError;

    #[test]
    fn only_an_encoding_error_leaves_the_connection_up() {
        assert!(!WStompError::StompEncoding(anyhow::anyhow!("bad frame")).is_fatal());
        for err in [
            WStompError::StompDecoding(anyhow::anyhow!("bad frame")),
            WStompError::WebsocketClosed(None),
            WStompError::WebsocketDropped,
            WStompError::PingTimeout,
        ] {
            assert!(err.is_fatal(), "{err}");
        }
    }
}
