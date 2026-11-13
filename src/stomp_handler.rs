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
    time::MissedTickBehavior,
};
use tokio_util::codec::{Decoder, Encoder};

use crate::{WStompError, wstomp_event::WStompEvent};

/// How many ping checks in a row may find nothing at all received since the
/// check before, before the connection counts as dead. Two, so that one late
/// pong is not taken for a dead connection.
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
///    connection when [`SILENT_INTERVALS`] checks in a row find nothing at all
///    received since the check before
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
    // pings late, but it is plainly still there. Silence is counted in ping
    // checks, not in time, so a stall of this task (e.g. waiting for an app
    // that stopped reading its events) is not taken for silence: a frame
    // received before the stall still counts at the check after it.
    // The connection has just been made, which counts as a sign of life.
    let mut received_since_check = true;
    let mut silent_checks = 0;

    'connection: loop {
        select! {
            // Branches are tried in this order. The app's frames come first,
            // so a flood from the server cannot hold back a SUBSCRIBE, ACK or
            // DISCONNECT. The ping check comes last, so frames that arrived
            // while this task was busy are read before silence is checked.
            biased;

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
                received_since_check = true;
                // STOMP frames need not line up with WebSocket messages: one
                // message may carry several, and a frame may span several
                // messages or fragments. So every payload goes into one
                // buffer, and whatever follows the last whole frame waits
                // there for the rest. Decoding parses the buffer from its
                // start, so it waits for the end of a message: a frame sent in
                // many fragments is parsed once, not once per fragment.
                let message_ended = match ws_frame {
                    WsFrame::Ping(bytes) => {
                        if let Err(e) = ws_sink.send(WsMessage::Pong(bytes)).await {
                            let _ = stomp_tx.send(WStompError::WsSend(e).into()).await;
                            break;
                        }
                        false
                    }
                    WsFrame::Text(bytes)
                    | WsFrame::Binary(bytes)
                    | WsFrame::Continuation(WsItem::Last(bytes)) => {
                        read_buf.extend_from_slice(&bytes);
                        true
                    }
                    WsFrame::Continuation(
                        WsItem::FirstText(bytes)
                        | WsItem::FirstBinary(bytes)
                        | WsItem::Continue(bytes),
                    ) => {
                        read_buf.extend_from_slice(&bytes);
                        false
                    }
                    WsFrame::Close(reason) => {
                        let _ = stomp_tx.send(WStompEvent::Error(WStompError::WebsocketClosed(reason))).await;
                        break;
                    }
                    // Counted as a sign of life above, like every other frame.
                    WsFrame::Pong(_) => false,
                };

                if !message_ended {
                    continue;
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

            _ = interval.tick() => {
                silent_checks = if received_since_check { 0 } else { silent_checks + 1 };
                received_since_check = false;
                if silent_checks >= SILENT_INTERVALS {
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
