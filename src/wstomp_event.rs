use async_stomp::{FromServer, Message, ToServer};
use awc::{
    error::{WsClientError, WsProtocolError},
    ws::CloseReason,
};
use tokio::sync::mpsc::error::SendError;

#[derive(Debug)]
pub enum WStompConnectError {
    WsClientError(WsClientError),
    ConnectMessageFailed(SendError<Message<ToServer>>),
}

/// Custom enum combine events in WebSocket and STOMP
#[derive(Debug)]
pub enum WStompEvent {
    /// Regular message from STOMP protocol
    Message(Message<FromServer>),
    /// Websocket closed connection (with reason)
    WebsocketClosed(Option<CloseReason>),
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
pub enum WStompError {
    /// Error during receiving websocket frames (from awc)
    WsReceive(WsProtocolError),
    /// Error during sending websocket frames (from awc)
    WsSend(WsProtocolError),
    /// Error while decoding (receiving) STOMP message (from async-stomp)
    StompDecoding(anyhow::Error),
    /// Error while encoding (sending) STOMP message (from async-stomp)
    StompEncoding(anyhow::Error),
    /// Incomplete STOMP frame received through WebSocket
    ///
    /// This is a warning that WebSocket protocol finished receiving data, but STOMP protocol
    /// doesn't recognize it as a full STOMP message. Should not happen, can be ignored in most cases.
    IncompleteStompFrame,
}

impl std::fmt::Display for WStompConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WsClientError(err) => write!(f, "WebSocket receive error: {}", err),
            Self::ConnectMessageFailed(msg) => write!(f, "WebSocket receive error: {}", msg),
        }
    }
}

impl std::error::Error for WStompConnectError {}

impl std::fmt::Display for WStompError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WsReceive(err) => write!(f, "WebSocket receive error: {}", err),
            Self::StompDecoding(err) => write!(f, "STOMP decoding error: {}", err),
            Self::StompEncoding(err) => write!(f, "STOMP encoding error: {}", err),
            Self::IncompleteStompFrame => {
                write!(f, "STOMP decoding warning: Dropped incomplete frame")
            }
            Self::WsSend(err) => write!(f, "WebSocket send error: {}", err),
        }
    }
}

impl std::error::Error for WStompError {}
