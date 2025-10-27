#![doc = pretty_readme::docify!("README.md", "https://docs.rs/wstomp/latest/wstomp/", "./")]

mod config;
pub use config::WStompConfig;

mod client;
pub use client::{WStompClient, WStompReceiver, WStompSender};

mod connect;
pub use connect::{StompConnect, connect, connect_with_pass, connect_with_token};

mod stomp_handler;

#[cfg(feature = "rustls")]
mod connect_ssl;
pub use connect_ssl::{connect_ssl, connect_ssl_with_pass, connect_ssl_with_token};

pub mod error;
pub use error::{WStompConnectError, WStompError};

// # Re-export stomp structs
pub mod stomp {
    pub use async_stomp::*;
}
