<!-- markdownlint-configure-file { "no-duplicate-heading": { "siblings_only": true } } -->

<!-- markdownlint-disable-next-line first-line-h1 -->
## 0.3.0 - Unreleased

### Added

* `WStompConfig::ping_interval` (and `WStompConfigOpts::ping_interval`) sets
  how often a WebSocket ping is sent; it was fixed at 20 seconds, which is
  still the default. It takes 1 second to a day.
* `WStompError::is_fatal`, whether the connection is gone after the error.

### Changed

* `WStompError`, `WStompEvent`, `WStompConnectError` and `WStompConfigOpts`
  are `#[non_exhaustive]`, so later versions can add variants and fields
  without breaking anyone. Match them with a wildcard arm, and start
  `WStompConfigOpts` from `WStompConfigOpts::default()` (or use the
  `WStompConfig` builder) instead of a struct literal.
* Any frame from the server counts as a sign of life, not only a pong, and
  `PingTimeout` comes only when two ping checks in a row find nothing
  received since the check before (it was one interval without a pong).
  Silence is counted in checks, not in time, and frames already waiting are
  read before a check, so an app that stops reading its events for a while
  is not taken for a silent server. Ticks missed during a stall are delayed,
  not fired back to back.
* A STOMP frame that grows past 16 MiB before it ends (it may now span
  several WebSocket messages) ends the connection as `StompDecoding`.

### Removed

* `WStompError::IncompleteStompFrame`: the start of a frame now waits for the
  rest of it in the next WebSocket message, so it is never reported.

### Fixed

* Every STOMP frame in a WebSocket message is delivered. Only the first was,
  and the rest were dropped without a word.
* A STOMP frame split across WebSocket messages is put back together. It was
  dropped as `IncompleteStompFrame`, which ended the connection.
* End-of-line heart-beats from the server are skipped. One sent alone ended
  the connection as `IncompleteStompFrame`, and two before a frame failed its
  decoding.
* Dropping the client, or its receiver, closes the connection. It stayed
  open, pinging, until an event could not be delivered, which on an idle
  subscription was never.
* A connection that ends without a Close frame, or fails with a WebSocket
  protocol error, is reported at once, as `WStompError::WebsocketDropped` or
  `WStompError::WsReceive`. Before, both went unnoticed until the ping check
  (20 to 40 seconds later) reported `PingTimeout`.
* A frame that fails to encode no longer leaves bytes behind to be sent with
  the next one.
* A frame that fails to go out is reported as `WStompError::WsSend`, not
  `WsReceive`.

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
