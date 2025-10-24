use actix_codec::Framed;
use actix_http::ws::{Codec, Frame as WsFrame, Item as WsItem, Message as WsMessage};
use async_stomp::{FromServer, Message, ToServer, client::ClientCodec};
use awc::{BoxedSocket, error::WsProtocolError};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender, error::SendError},
};
use tokio_util::codec::{Decoder, Encoder};

/// Custom error type to combine WebSocket and STOMP errors.
#[derive(Debug)]
pub enum WstompError {
    WsReceive(WsProtocolError),
    Stomp(anyhow::Error),
    IncompleteStompFrame,
    WsSend(WsProtocolError),
}

impl std::fmt::Display for WstompError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WsReceive(e) => write!(f, "WebSocket receive error: {}", e),
            Self::Stomp(e) => write!(f, "STOMP decoding error: {}", e),
            Self::IncompleteStompFrame => {
                write!(f, "STOMP decoding warning: Dropped incomplete frame")
            }
            Self::WsSend(e) => write!(f, "WebSocket send error: {}", e),
        }
    }
}
impl std::error::Error for WstompError {}

type WsFramed = Framed<BoxedSocket, Codec>;

/// Your client which reads websocket and produces STOMP messages. Also takes STOMP messages from you and sends it through websocket
pub struct WstompClient {
    /// Send STOMP frames to the server with this.
    pub tx: Sender<Message<ToServer>>,
    /// Receive STOMP frames from the server with this.
    pub rx: Receiver<Result<Message<FromServer>, WstompError>>,
}

impl WstompClient {
    /// Creates a new STOMP client based on a websocket connection made by awc client.
    ///
    /// Pass the `Framed` object you get from `awc` directly into this constructor.
    /// This will create a background worker in actix system, which will encode and decode STOMP messages for you.
    /// It also manages websocket ping-pong heartbeat.
    pub fn new(ws_framed: Framed<BoxedSocket, Codec>) -> Self {
        // Channel for you to send STOMP frames to the handler task
        let (app_tx, app_rx) = mpsc::channel::<Message<ToServer>>(100);

        // Channel for the handler task to send STOMP frames back to you
        let (stomp_tx, stomp_rx) = mpsc::channel::<Result<Message<FromServer>, WstompError>>(100);

        // Spawn the task that handles all the low-level logic.
        actix_rt::spawn(stomp_handler_task(ws_framed, app_rx, stomp_tx));

        Self {
            tx: app_tx,
            rx: stomp_rx,
        }
    }

    pub async fn recv<T>(&mut self) -> Option<Result<Message<FromServer>, WstompError>> {
        self.rx.recv().await
    }

    pub async fn send<T>(
        &mut self,
        value: Message<ToServer>,
    ) -> Result<(), SendError<Message<ToServer>>> {
        self.tx.send(value).await
    }
}

/// This is the internal task that manages the connection.
/// It multiplexes between:
/// 1. Receiving WebSocket messages (handling Pings, decoding STOMP)
/// 2. Receiving STOMP frames from your app (encoding, sending)
/// 3. Sending ping WebSocket frames every 20 seconds
async fn stomp_handler_task(
    ws_framed: WsFramed,
    mut app_rx: Receiver<Message<ToServer>>,
    stomp_tx: Sender<Result<Message<FromServer>, WstompError>>,
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
                            let _ = stomp_tx.send(Err(WstompError::WsSend(e))).await;
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
                            let _ = stomp_tx.send(Err(WstompError::IncompleteStompFrame)).await;
                            break;
                        }
                        Err(e) => {
                            // STOMP decoding error
                            let _ = stomp_tx.send(Err(WstompError::Stomp(e))).await;
                            break;
                        }
                    }
                }
            }

            // Received a STOMP frame from the application (client) to send
            Some(stomp_frame_to_send) = app_rx.recv() => {
                println!("---> {:?}", stomp_frame_to_send);
                // Encode the STOMP frame
                match stomp_codec.encode(stomp_frame_to_send, &mut encode_buf) {
                    Ok(_) => {
                        // Send it as a WebSocket Binary message
                        if let Err(e) = ws_sink.send(WsMessage::Binary(encode_buf.clone().freeze())).await {
                            let _ = stomp_tx.send(Err(WstompError::WsReceive(e))).await;
                            break;
                        }
                        encode_buf.clear();
                    }
                    Err(e) => {
                        // STOMP encoding error
                        let _ = stomp_tx.send(Err(WstompError::Stomp(e))).await;
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
