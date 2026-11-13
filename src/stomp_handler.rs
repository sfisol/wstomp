use actix_codec::Framed;
use actix_http::ws::{Codec, Frame as WsFrame, Item as WsItem, Message as WsMessage};
use async_stomp::{Message, ToServer, client::ClientCodec};
use awc::BoxedSocket;
use bytes::{Bytes, BytesMut};
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

    loop {
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
                let mut finished_reading = false;
                match ws_frame {
                    WsFrame::Ping(bytes) => {
                        if let Err(e) = ws_sink.send(WsMessage::Pong(bytes)).await {
                            let _ = stomp_tx.send(WStompError::WsSend(e).into()).await;
                            break;
                        }
                    }
                    WsFrame::Text(text) => {
                        read_buf.extend_from_slice(&text);
                        finished_reading = true;
                    }
                    WsFrame::Binary(bytes) => {
                        read_buf.extend_from_slice(&bytes);
                        finished_reading = true;
                    }
                    WsFrame::Close(reason) => {
                        let _ = stomp_tx.send(WStompEvent::Error(WStompError::WebsocketClosed(reason))).await;
                        break;
                    }
                    // Counted as a sign of life above, like every other frame.
                    WsFrame::Pong(_) => {}
                    WsFrame::Continuation(item) => {
                        match item {
                            WsItem::FirstText(bytes) => {
                                read_buf.clear();
                                read_buf.extend_from_slice(&bytes);
                            },
                            WsItem::FirstBinary(bytes) => {
                                read_buf.clear();
                                read_buf.extend_from_slice(&bytes);
                            },
                            WsItem::Continue(bytes) => {
                                read_buf.extend_from_slice(&bytes);
                            },
                            WsItem::Last(bytes) => {
                                read_buf.extend_from_slice(&bytes);
                                finished_reading = true;
                            },
                        }
                    }
                }

                // After receiving data, try to decode STOMP frames
                if finished_reading {
                    match stomp_codec.decode(&mut read_buf) {
                        Ok(Some(stomp_frame)) => {
                            read_buf.clear();
                            // Decoded a STOMP frame, send it to the app
                            if stomp_tx.send(WStompEvent::Message(stomp_frame)).await.is_err() {
                                // Receiver was dropped, app is gone.
                                break;
                            }
                        }
                        Ok(None) => {
                            // Not enough data for a full STOMP frame, wait for more.
                            let _ = stomp_tx.send(WStompError::IncompleteStompFrame.into()).await;
                            break;
                        }
                        Err(e) => {
                            // STOMP decoding error
                            let _ = stomp_tx.send(WStompError::StompDecoding(e).into()).await;
                            break;
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
