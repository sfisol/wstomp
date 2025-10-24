mod client;
pub use client::{WstompClient, WstompError};

mod connect;
pub use connect::{
    StompConnect, WstompConnectError, connect, connect_with_options, connect_with_pass,
    connect_with_token,
};

#[cfg(feature = "rustls")]
mod connect_ssl;
pub use connect_ssl::{connect_ssl, connect_ssl_with_pass, connect_ssl_with_token};

// # Re-export usual stomp structs
pub use async_stomp::{FromServer, Message, ToServer};
