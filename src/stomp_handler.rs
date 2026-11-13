use actix_codec::Framed;
use actix_http::ws::{Codec, Frame as WsFrame, Item as WsItem, Message as WsMessage};
use async_stomp::{FromServer, Message, ToServer, client::ClientCodec};
use awc::BoxedSocket;
use bytes::{Buf, Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::{
    select,
    sync::mpsc::{Receiver, Sender},
    time::{Instant, MissedTickBehavior},
};
use tokio_util::codec::{Decoder, Encoder};

use crate::{WStompError, wstomp_event::WStompEvent};

/// How many ping intervals may pass with nothing at all received before the
/// connection counts as dead. Two, so that one late pong is not taken for a
/// dead connection.
const SILENT_INTERVALS: u32 = 2;

/// How large the start of a STOMP frame may grow while the rest of it is
/// awaited. Far above any real frame: it only stops a stream that never ends
/// its frame from growing the buffer without limit.
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// This is the internal task that manages the connection.
/// It multiplexes between:
/// 1. Receiving WebSocket messages (handling Pings, decoding STOMP)
/// 2. Receiving STOMP frames from your app (encoding, sending)
/// 3. Sending ping WebSocket frames every `ping_interval`, and ending the
///    connection when nothing at all has arrived for [`SILENT_INTERVALS`] of them
pub(crate) async fn stomp_handler_task(
    ws_framed: Framed<BoxedSocket, Codec>,
    mut app_rx: Receiver<Message<ToServer>>,
    stomp_tx: Sender<WStompEvent>,
    ping_interval: Duration,
) {
    let (mut ws_sink, mut ws_stream) = ws_framed.split();
    let mut stomp_codec = ClientCodec;
    let mut read_buf = BytesMut::new();
    let mut encode_buf = BytesMut::new();

    let mut interval = actix_rt::time::interval(ping_interval);
    // After a stall, the missed ticks must not fire back to back: each would
    // check for silence again before a pong could have arrived.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Any frame is a sign of life, not only a pong: a busy server may answer
    // pings late, but it is plainly still there.
    let silence_limit = ping_interval * SILENT_INTERVALS;
    let mut last_received = Instant::now();

    'connection: loop {
        select! {
            // Received a message from the WebSocket server. The end of the
            // stream and a receive error must be matched here: a pattern that
            // fails to match only disables the branch until the next loop, and
            // the loss would surface at the ping check, up to two intervals later.
            ws_frame = ws_stream.next() => {
                let ws_frame = match ws_frame {
                    Some(Ok(ws_frame)) => ws_frame,
                    Some(Err(e)) => {
                        let _ = stomp_tx.send(WStompError::WsReceive(e).into()).await;
                        break;
                    }
                    None => {
                        let _ = stomp_tx.send(WStompError::WebsocketDropped.into()).await;
                        break;
                    }
                };
                last_received = Instant::now();
                match ws_frame {
                    WsFrame::Ping(bytes) => {
                        if let Err(e) = ws_sink.send(WsMessage::Pong(bytes)).await {
                            let _ = stomp_tx.send(WStompError::WsSend(e).into()).await;
                            break;
                        }
                    }
                    // STOMP frames need not line up with WebSocket messages:
                    // one message may carry several, and a frame may span
                    // several messages or fragments. So every payload goes
                    // into one buffer, and whatever follows the last whole
                    // frame waits there for the rest.
                    WsFrame::Text(bytes)
                    | WsFrame::Binary(bytes)
                    | WsFrame::Continuation(
                        WsItem::FirstText(bytes)
                        | WsItem::FirstBinary(bytes)
                        | WsItem::Continue(bytes)
                        | WsItem::Last(bytes),
                    ) => read_buf.extend_from_slice(&bytes),
                    WsFrame::Close(reason) => {
                        let _ = stomp_tx.send(WStompEvent::Error(WStompError::WebsocketClosed(reason))).await;
                        break;
                    }
                    // Counted as a sign of life above, like every other frame.
                    WsFrame::Pong(_) => {}
                }

                loop {
                    match next_stomp_frame(&mut stomp_codec, &mut read_buf) {
                        Ok(Some(stomp_frame)) => {
                            if stomp_tx.send(WStompEvent::Message(stomp_frame)).await.is_err() {
                                // Receiver was dropped, app is gone.
                                break 'connection;
                            }
                        }
                        // Nothing left, or only the start of a frame.
                        Ok(None) => break,
                        Err(e) => {
                            let _ = stomp_tx.send(WStompError::StompDecoding(e).into()).await;
                            break 'connection;
                        }
                    }
                }
            }

            // Received a STOMP frame from the application (client) to send
            Some(stomp_frame_to_send) = app_rx.recv() => {
                // Encode the STOMP frame
                match stomp_codec.encode(stomp_frame_to_send, &mut encode_buf) {
                    Ok(_) => {
                        // Send it as a WebSocket Binary message
                        if let Err(e) = ws_sink.send(WsMessage::Binary(encode_buf.clone().freeze())).await {
                            let _ = stomp_tx.send(WStompError::WsReceive(e).into()).await;
                            break;
                        }
                        encode_buf.clear();
                    }
                    Err(e) => {
                        // STOMP encoding error. Only this frame is lost, so the
                        // connection stays up; nothing of it may go out with the next.
                        encode_buf.clear();
                        let _ = stomp_tx.send(WStompError::StompEncoding(e).into()).await;
                    }
                }
            }

            _ = interval.tick() => {
                if last_received.elapsed() >= silence_limit {
                    let _ = stomp_tx.send(WStompEvent::Error(WStompError::PingTimeout)).await;
                    break;
                }
                match ws_sink.send(WsMessage::Ping(Bytes::from_static(b"wstomp"))).await {
                    Ok(_) => {}
                    Err(err) => {
                        let _ = stomp_tx.send(WStompEvent::Error(WStompError::PingFailed(err.into()))).await;
                        break;
                    }
                }
            }

            // 3. Both streams closed, exit loop
            else => break,
        }
    }
}

/// Takes the next whole STOMP frame off the front of `buf`, skipping the
/// end-of-line heart-beats a server sends between frames. `Ok(None)` means
/// `buf` holds nothing more, or only the start of a frame.
fn next_stomp_frame(
    codec: &mut ClientCodec,
    buf: &mut BytesMut,
) -> anyhow::Result<Option<Message<FromServer>>> {
    let heart_beats = buf
        .iter()
        .take_while(|byte| matches!(byte, b'\n' | b'\r'))
        .count();
    buf.advance(heart_beats);

    let frame = codec.decode(buf)?;
    if frame.is_none() && buf.len() > MAX_FRAME_SIZE {
        anyhow::bail!("STOMP frame larger than {MAX_FRAME_SIZE} bytes");
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use async_stomp::client::ClientCodec;
    use bytes::BytesMut;

    use super::{MAX_FRAME_SIZE, next_stomp_frame};

    #[test]
    fn a_frame_that_outgrows_the_limit_is_an_error() {
        let mut buf = BytesMut::from(&b"MESSAGE\ndestination:/a\n\n"[..]);
        buf.resize(MAX_FRAME_SIZE + 1, b'a');

        assert!(next_stomp_frame(&mut ClientCodec, &mut buf).is_err());
    }

    #[test]
    fn heart_beats_alone_leave_nothing_behind() {
        let mut buf = BytesMut::from(&b"\n\r\n\n"[..]);

        assert!(
            next_stomp_frame(&mut ClientCodec, &mut buf)
                .unwrap()
                .is_none()
        );
        assert!(buf.is_empty());
    }
}
