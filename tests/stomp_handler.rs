//! The handler task against a bare WebSocket server, which completes the
//! handshake and then misbehaves in one particular way.

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use wstomp::{WStompClient, WStompError, WStompEvent};

/// Well below the 20 s ping interval, so an event within it comes from the
/// handler noticing the socket, not from a ping check.
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);

/// Connects a client to a fresh server and returns both ends.
async fn connect() -> (WStompClient, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());

    let (server, client) = tokio::join!(accept(&listener), awc::Client::new().ws(url).connect());
    let (_response, framed) = client.unwrap();

    (WStompClient::from_framed(framed), server)
}

/// Completes the server side of the WebSocket handshake.
async fn accept(listener: &TcpListener) -> TcpStream {
    let (mut stream, _) = listener.accept().await.unwrap();

    let mut request = Vec::new();
    let mut buf = [0; 1024];
    while !request.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut buf).await.unwrap();
        assert!(read > 0, "client closed during the handshake");
        request.extend_from_slice(&buf[..read]);
    }

    let request = String::from_utf8(request).unwrap();
    let key = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("sec-websocket-key")
                .then(|| value.trim().to_owned())
        })
        .unwrap();
    let accept = actix_http::ws::hash_key(key.as_bytes());

    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        std::str::from_utf8(&accept).unwrap()
    );
    stream.write_all(response.as_bytes()).await.unwrap();

    stream
}

async fn next_event(client: &mut WStompClient) -> WStompEvent {
    timeout(EVENT_TIMEOUT, client.recv())
        .await
        .expect("no event before the timeout")
        .expect("event channel closed")
}

#[actix_rt::test]
async fn reports_a_socket_closed_without_a_close_frame() {
    let (mut client, mut server) = connect().await;

    // Ends the stream without a Close frame (code 1006 on the other side).
    // The read half stays open, so the client's pings do not reset it.
    server.shutdown().await.unwrap();

    let event = next_event(&mut client).await;
    assert!(
        matches!(event, WStompEvent::Error(WStompError::WebsocketDropped)),
        "{event:?}"
    );
}

#[actix_rt::test]
async fn reports_a_websocket_protocol_error() {
    let (mut client, mut server) = connect().await;

    // A frame with the reserved opcode 0x3.
    server.write_all(&[0x83, 0x00]).await.unwrap();

    let event = next_event(&mut client).await;
    assert!(
        matches!(event, WStompEvent::Error(WStompError::WsReceive(_))),
        "{event:?}"
    );
}

#[actix_rt::test]
async fn reports_a_close_frame() {
    let (mut client, mut server) = connect().await;

    // Close with code 1000.
    server.write_all(&[0x88, 0x02, 0x03, 0xE8]).await.unwrap();

    let event = next_event(&mut client).await;
    assert!(
        matches!(
            event,
            WStompEvent::Error(WStompError::WebsocketClosed(Some(_)))
        ),
        "{event:?}"
    );
}
