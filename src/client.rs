use actix_codec::Framed;
use actix_http::ws::{Codec, Frame as WsFrame, Item as WsItem, Message as WsMessage};
use async_stomp::{FromServer, Message, ToServer, client::ClientCodec};
use awc::BoxedSocket;
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender, error::SendError},
};
use tokio_util::codec::{Decoder, Encoder};

use crate::WStompError;

pub type WStompSender = Sender<Message<ToServer>>;
pub type WStompReceiver = Receiver<Result<Message<FromServer>, WStompError>>;

/// Your client which reads websocket and produces STOMP messages. Also takes STOMP messages from you and sends it through websocket
pub struct WStompClient {
    /// Send STOMP frames to the server with this.
    tx: WStompSender,
    /// Receive STOMP frames from the server with this.
    rx: WStompReceiver,
}

impl WStompClient {
    /// Creates a new STOMP client based on a websocket connection made by awc client.
    ///
    /// You can use this struct directly by passing the `Framed` object you get from `awc` into this constructor.
    /// This will create a background worker in actix system (on current thread), which will encode and decode STOMP messages for you.
    /// It also manages websocket ping-pong heartbeat.
    pub fn new(ws_framed: Framed<BoxedSocket, Codec>) -> Self {
        // Channel for you to send STOMP frames to the handler task
        let (app_tx, app_rx) = mpsc::channel::<Message<ToServer>>(100);

        // Channel for the handler task to send STOMP frames back to you
        let (stomp_tx, stomp_rx) = mpsc::channel::<Result<Message<FromServer>, WStompError>>(100);

        // Spawn the task that handles all the low-level logic.
        actix_rt::spawn(stomp_handler_task(ws_framed, app_rx, stomp_tx));

        Self {
            tx: app_tx,
            rx: stomp_rx,
        }
    }

    pub async fn recv(&mut self) -> Option<Result<Message<FromServer>, WStompError>> {
        self.rx.recv().await
    }

    pub async fn send(&self, value: Message<ToServer>) -> Result<(), SendError<Message<ToServer>>> {
        self.tx.send(value).await
    }

    pub fn into_split(self) -> (WStompReceiver, WStompSender) {
        (self.rx, self.tx)
    }
}

/// This is the internal task that manages the connection.
/// It multiplexes between:
/// 1. Receiving WebSocket messages (handling Pings, decoding STOMP)
/// 2. Receiving STOMP frames from your app (encoding, sending)
/// 3. Sending ping WebSocket frames every 20 seconds
async fn stomp_handler_task(
    ws_framed: Framed<BoxedSocket, Codec>,
    mut app_rx: Receiver<Message<ToServer>>,
    stomp_tx: Sender<Result<Message<FromServer>, WStompError>>,
) {
    let (mut ws_sink, mut ws_stream) = ws_framed.split();
    let mut stomp_codec = ClientCodec;
    let mut read_buf = BytesMut::new();
    let mut encode_buf = BytesMut::new();

    let mut interval = actix_rt::time::interval(Duration::from_secs(20));

    loop {
        select! {
            // Received a message from the WebSocket server
            Some(Ok(ws_frame)) = ws_stream.next() => {
                let mut finished_reading = false;
                match ws_frame {
                    WsFrame::Ping(bytes) => {
                        if let Err(e) = ws_sink.send(WsMessage::Pong(bytes)).await {
                            let _ = stomp_tx.send(Err(WStompError::WsSend(e))).await;
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
                        println!("Server closed WebSocket: {:?}", reason);
                        break;
                    }
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
                            if stomp_tx.send(Ok(stomp_frame)).await.is_err() {
                                // Receiver was dropped, app is gone.
                                break;
                            }
                        }
                        Ok(None) => {
                            // Not enough data for a full STOMP frame, wait for more.
                            let _ = stomp_tx.send(Err(WStompError::IncompleteStompFrame)).await;
                            break;
                        }
                        Err(e) => {
                            // STOMP decoding error
                            let _ = stomp_tx.send(Err(WStompError::StompDecoding(e))).await;
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
                            let _ = stomp_tx.send(Err(WStompError::WsReceive(e))).await;
                            break;
                        }
                        encode_buf.clear();
                    }
                    Err(e) => {
                        // STOMP encoding error
                        let _ = stomp_tx.send(Err(WStompError::StompEncoding(e))).await;
                    }
                }
            }

            _ = interval.tick() => {
                let _ = ws_sink.send(WsMessage::Ping(Bytes::from_static(b"wstomp"))).await;
            }

            // 3. Both streams closed, exit loop
            else => {
                println!("STOMP client task shutting down.");
                break;
            }
        }
    }
}
