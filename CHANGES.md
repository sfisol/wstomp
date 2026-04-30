<!-- markdownlint-configure-file { "no-duplicate-heading": { "siblings_only": true } } -->

<!-- markdownlint-disable-next-line first-line-h1 -->
## 0.2.0 - 2026-04-30

### Added

* Reconnect with callback installed via `WStompConfig::on_reconnect`.
  The reconnect loop is then spawned by `WStompConfig::build_and_connect`,
  which in the reconnecting state returns a `WStompReconnectHandle`
  instead of a `WStompClient`.
* Certificate authentication - `connect_ssl_with_cert`.

## 0.1.0 - 2025-11-18

* Initial release
* Connects to STOMP servers over WebSocket using [`awc`](https://crates.io/crates/awc).
* Handles all STOMP protocol encoding and decoding via [`async-stomp`](https://crates.io/crates/async-stomp).
* Manages WebSocket ping/pong heartbeats automatically in a background task.
* Provides a simple `tokio::mpsc` channel-based API ([`WStompClient`]) for sending and receiving STOMP frames.
* Connection helpers for various authentication methods
* Optional `rustls` feature for SSL connections, with helpers that force HTTP/1.1 for compatibility with servers like SockJS.
