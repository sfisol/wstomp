# wstomp

A STOMP-over-WebSocket client library for Rust, built on top of `awc` and `async-stomp`.

This crate provides a simple client to connect to a STOMP-enabled WebSocket server (like RabbitMQ over Web-STOMP, or ActiveMQ). It handles the WebSocket connection, STOMP frame encoding/decoding, and WebSocket heartbeat (ping/pong) for you.

## Features

* Connects to STOMP servers over WebSocket using [`awc`](https://crates.io/crates/awc).
* Handles all STOMP protocol encoding and decoding via [`async-stomp`](https://crates.io/crates/async-stomp).
* Manages WebSocket ping/pong heartbeats automatically in a background task.
* Provides a simple `tokio::mpsc` channel-based API (`WStompClient`) for sending and receiving STOMP frames.
* Connection helpers for various authentication methods:
  * `connect`: Anonymous connection.
  * `connect_with_pass`: Login and passcode authentication.
  * `connect_with_token`: Authentication using an authorization token header.
* Optional `rustls` feature for SSL connections, with helpers that force HTTP/1.1 for compatibility with servers like SockJS.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
wstomp = "0.1.0" # Replace with the actual version
actix-rt = "2.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures-util = "0.3"
```

For SSL support, enable the `rustls` feature:

```toml
[dependencies]
wstomp = { version = "0.1.0", features = ["rustls"] }
```

## Usage

Here is a basic example of connecting, subscribing to a topic, and receiving messages.

```rust,no_run
use wstomp::{
    connect_with_pass,
    stomp::{FromServer, Message, ToServer, client::Subscriber},
    WStompError,
};

#[actix_rt::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = "ws://127.0.0.1:15674/ws/websocket"; // Example: RabbitMQ Web-STOMP (Note the "/websocket" suffix)
    let login = "guest".to_string();
    let passcode = "guest".to_string();

    // 1. Connect to the server
    println!("Connecting to {}...", url);
    let mut client = connect_with_pass(url, login, passcode)
        .await
        .expect("Failed to connect");

    println!("Connected! Subscribing...");

    // 2. Create a SUBSCRIBE frame
    let subscribe_frame = Subscriber::builder()
        .destination("queue.test")
        .id("subscription-1")
        .subscribe();

    // 3. Send the SUBSCRIBE frame
    client.send(subscribe_frame).await?;

    println!("Subscribed! Waiting for messages...");

    // 4. Listen for incoming messages
    loop {
        match client.recv().await {
            // Receive messages from the server
            Some(Ok(msg)) => {
                match msg.content {
                    FromServer::Message {
                        destination,
                        message_id,
                        subscription,
                        body,
                        ..
                    } => {
                        println!("\n--- NEW MESSAGE ---");
                        println!("Destination: {}", destination);
                        println!("Subscription: {}", subscription);
                        println!("Message ID: {}", message_id);
                        if let Some(body_bytes) = body {
                            println!("Body: {}", String::from_utf8_lossy(&body_bytes));
                        }
                    }
                    FromServer::Receipt { receipt_id } => {
                        println!("Received receipt: {}", receipt_id);
                    }
                    FromServer::Connected { .. } => {
                        println!("Received CONNECTED frame (usually the first message)");
                    }
                    FromServer::Error { message, body, .. } => {
                        println!("Received ERROR frame: {}", message.unwrap_or_default());
                        if let Some(body_bytes) = body {
                            println!("Error Body: {}", String::from_utf8_lossy(&body_bytes));
                        }
                        break;
                    }
                    // IncompleteStompFrame is a warning, not a hard error
                    other => println!("Received other frame: {:?}", other),
                }
            }
            // Handle errors
            Some(Err(e)) => {
                 match e {
                    WStompError::IncompleteStompFrame => {
                        // This is a warning, you can choose to ignore it or log it
                        eprintln!("Warning: Dropped incomplete STOMP frame.");
                    }
                    other_err => {
                        // These are more serious errors
                        eprintln!("Connection error: {}", other_err);
                        break;
                    }
                }
            }
            // Exit loop if channel closes
            None => break,
        }
    }

    Ok(())
}
```

### Connection with an Auth Token

If you need to pass an `Authorization` header (or any custom header):

```rust,no_run
use wstomp::connect_with_token;

#[actix_rt::main]
async fn main() {
    let url = "ws://my-server.com/ws/websocket";
    let token = "my-secret-jwt-token";

    let client = connect_with_token(url, token)
        .await
        .expect("Failed to connect");

    // ... use client
}
```

### Connection with SSL (`rustls` feature)

If you are connecting to a `wss://` endpoint and need SSL, use the `rustls` feature and the `connect_ssl_*` helpers.

These helpers are specially configured to force HTTP/1.1, which can be necessary for compatibility with some WebSocket servers (like those using SockJS).

```rust,no_run
// Make sure to enable the "rustls" feature in Cargo.toml
use wstomp::connect_ssl_with_pass;

#[actix_rt::main]
async fn main() {
    let url = "wss://secure-server.com/ws";
    let login = "user".to_string();
    let passcode = "pass".to_string();

    let client = connect_ssl_with_pass(url, login, passcode)
        .await
        .expect("Failed to connect with SSL");

    println!("Connected securely!");
    // ... use client
}
```

## Error Handling

The connection functions (`connect`, `connect_ssl`, etc.) return a `Result<WStompClient, WStompConnectError>`.

Once connected, the `WStompClient::rx` channel produces `Result<Message<FromServer>, WStompError>` items. You should check for errors in your receive loop.

* **`WStompConnectError`**: An error that occurs during the initial WebSocket and STOMP `CONNECT` handshake.
* **`WStompError`**: An error that occurs *after* a successful connection.
  * `WsReceive` / `WsSend`: A WebSocket protocol error.
  * `StompDecoding` / `StompEncoding`: A STOMP frame decoding/encoding error.
  * `IncompleteStompFrame`: A warning indicating that data was received but was not enough to form a complete STOMP frame. The client has dropped this data. This is often safe to ignore or log as a warning.

## License

This crate is licensed under "MIT" or "Apache-2.0".
