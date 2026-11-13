//! The handler task against a bare WebSocket server, which completes the
//! handshake and then misbehaves in one particular way.

use std::time::{Duration, Instant};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use wstomp::{
    WStompClient, WStompConfig, WStompError, WStompEvent,
    stomp::{FromServer, Message, client::Subscriber},
};

/// Well below the 20 s ping interval, so an event within it comes from the
/// handler noticing the socket, not from a ping check.
const EVENT_TIMEOUT: Duration = Duration::from_secs(4);

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

/// Sends `payload` as one unmasked WebSocket frame with `opcode`, the last
/// fragment of its message when `fin`.
async fn send_frame(server: &mut TcpStream, opcode: u8, fin: bool, payload: &[u8]) {
    let mut frame = vec![if fin { 0x80 | opcode } else { opcode }];
    match u8::try_from(payload.len()) {
        Ok(len) if len < 126 => frame.push(len),
        // A 16-bit length is enough for these tests.
        _ => {
            frame.push(126);
            frame.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    server.write_all(&frame).await.unwrap();
}

/// Sends `payload` as one unfragmented text message.
async fn send_text(server: &mut TcpStream, payload: &str) {
    send_frame(server, 0x1, true, payload.as_bytes()).await;
}

fn receipt(id: &str) -> String {
    format!("RECEIPT\nreceipt-id:{id}\n\n\0")
}

async fn expect_receipt(client: &mut WStompClient, id: &str) {
    let event = next_event(client).await;
    assert!(
        matches!(
            &event,
            WStompEvent::Message(Message { content: FromServer::Receipt { receipt_id }, .. })
                if receipt_id == id
        ),
        "expected receipt {id}, got {event:?}"
    );
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

#[actix_rt::test]
async fn reports_a_server_that_stops_answering_pings() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());

    // The server never answers, so after two intervals of silence (two
    // seconds) the connection is taken for dead.
    let config = WStompConfig::new(url).ping_interval(1);
    let (_server, client) = tokio::join!(accept(&listener), config.build_and_connect());
    let mut client = client.unwrap();

    let event = next_event(&mut client).await;
    assert!(
        matches!(event, WStompEvent::Error(WStompError::PingTimeout)),
        "{event:?}"
    );
}

#[actix_rt::test]
async fn any_frame_from_the_server_counts_as_a_sign_of_life() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());

    let config = WStompConfig::new(url).ping_interval(1);
    let (mut server, client) = tokio::join!(accept(&listener), config.build_and_connect());
    let mut client = client.unwrap();

    // The server never answers a ping, but keeps sending pings of its own.
    actix_rt::spawn(async move {
        loop {
            if server.write_all(&[0x89, 0x00]).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    // Well past the two silent intervals a missing pong alone would allow.
    let event = timeout(Duration::from_millis(3500), client.recv()).await;
    assert!(event.is_err(), "{event:?}");
}

#[actix_rt::test]
async fn delivers_every_frame_of_a_message() {
    let (mut client, mut server) = connect().await;

    send_text(&mut server, &format!("{}{}", receipt("1"), receipt("2"))).await;

    expect_receipt(&mut client, "1").await;
    expect_receipt(&mut client, "2").await;
}

#[actix_rt::test]
async fn puts_together_a_frame_split_across_messages() {
    let (mut client, mut server) = connect().await;

    let (first, second) = (receipt("1"), receipt("2"));
    let (first_start, first_rest) = first.split_at(10);
    let (second_start, second_rest) = second.split_at(12);
    send_text(&mut server, first_start).await;
    send_text(&mut server, &format!("{first_rest}{second_start}")).await;
    send_text(&mut server, second_rest).await;

    expect_receipt(&mut client, "1").await;
    expect_receipt(&mut client, "2").await;
}

#[actix_rt::test]
async fn puts_together_a_frame_split_across_fragments() {
    let (mut client, mut server) = connect().await;

    let frame = receipt("1");
    let (start, rest) = frame.split_at(10);
    // A text message in two fragments: a text frame without FIN, then a
    // continuation frame with it.
    send_frame(&mut server, 0x1, false, start.as_bytes()).await;
    send_frame(&mut server, 0x0, true, rest.as_bytes()).await;

    expect_receipt(&mut client, "1").await;
}

#[actix_rt::test]
async fn skips_heart_beats_between_frames() {
    let (mut client, mut server) = connect().await;

    send_text(&mut server, "\n").await;
    send_text(&mut server, &format!("{}\n", receipt("1"))).await;
    send_text(&mut server, "\r\n").await;
    send_text(&mut server, &format!("\n\n{}", receipt("2"))).await;
    send_text(&mut server, "\n").await;
    send_text(&mut server, &receipt("3")).await;

    // Events arrive in order, so no error came in between.
    expect_receipt(&mut client, "1").await;
    expect_receipt(&mut client, "2").await;
    expect_receipt(&mut client, "3").await;
}

#[actix_rt::test]
async fn an_app_that_falls_behind_is_not_taken_for_silence() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());

    let config = WStompConfig::new(url).ping_interval(1);
    let (mut server, client) = tokio::join!(accept(&listener), config.build_and_connect());
    let mut client = client.unwrap();

    // More frames than the event channel holds, so the handler waits to
    // deliver one while the app sleeps through more than two ping intervals.
    let receipts: String = (0..150).map(|id| receipt(&id.to_string())).collect();
    send_text(&mut server, &receipts).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    for id in 0..150 {
        expect_receipt(&mut client, &id.to_string()).await;
    }
    // The connection is still up.
    send_text(&mut server, &receipt("150")).await;
    expect_receipt(&mut client, "150").await;
}

#[actix_rt::test]
async fn takes_a_ping_interval_too_long_to_add_to_a_clock() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());

    let config = WStompConfig::new(url).ping_interval(u64::MAX);
    let (mut server, client) = tokio::join!(accept(&listener), config.build_and_connect());
    let mut client = client.unwrap();

    // The handler task is still there to deliver it.
    send_text(&mut server, &receipt("1")).await;
    expect_receipt(&mut client, "1").await;
}

#[actix_rt::test]
async fn closes_the_connection_when_the_client_is_dropped() {
    let (client, mut server) = connect().await;

    drop(client);

    let closed = timeout(EVENT_TIMEOUT, async {
        let mut buf = [0; 64];
        while matches!(server.read(&mut buf).await, Ok(read) if read > 0) {}
    })
    .await;
    assert!(closed.is_ok(), "the connection is still open");
}

#[actix_rt::test]
async fn parses_a_frame_in_many_fragments_once() {
    let (mut client, mut server) = connect().await;

    // A 4 MiB frame in 512-byte fragments. Parsed after every fragment, the
    // buffer would be scanned 8192 times, some 16 GiB in all.
    let mut frame = b"RECEIPT\nreceipt-id:1\n\n".to_vec();
    frame.resize(frame.len() + 4 * 1024 * 1024, b'a');
    frame.push(0);
    let mut fragments = frame.chunks(512).peekable();

    let started = Instant::now();
    let mut opcode = 0x1;
    while let Some(fragment) = fragments.next() {
        send_frame(&mut server, opcode, fragments.peek().is_none(), fragment).await;
        opcode = 0x0;
    }
    expect_receipt(&mut client, "1").await;
    // Sending and parsing it once takes well under a second.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[actix_rt::test]
async fn reports_a_frame_that_cannot_be_sent() {
    let (client, mut server) = connect().await;
    let subscribe = || {
        Subscriber::builder()
            .destination("/queue/a")
            .id("1")
            .subscribe()
    };

    // The server closes with the first frame unread, which resets the
    // connection, so the second one cannot go out.
    client.send(subscribe()).await.unwrap();
    server.peek(&mut [0]).await.unwrap();
    drop(server);
    // The test and the handler share one thread: blocking it lets the reset
    // arrive before the handler runs again, and the handler then takes the
    // app's frame before anything else.
    std::thread::sleep(Duration::from_millis(100));
    client.send(subscribe()).await.unwrap();

    let mut client = client;
    let event = next_event(&mut client).await;
    assert!(
        matches!(event, WStompEvent::Error(WStompError::WsSend(_))),
        "{event:?}"
    );
}
