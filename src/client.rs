use actix_codec::Framed;
use actix_http::Uri;
use async_stomp::{FromServer, Message, ToServer};
use awc::{BoxedSocket, error::HttpError, ws::Codec};
use tokio::sync::mpsc::{self, Receiver, Sender, error::SendError};

use crate::{WStompConfig, WStompError, stomp_handler::stomp_handler_task};

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
    pub fn builder<U>(url: U) -> WStompConfig<U>
    where
        Uri: TryFrom<U>,
        <Uri as TryFrom<U>>::Error: Into<HttpError>,
    {
        WStompConfig::new(url)
    }

    /// Creates a new STOMP client based on a websocket connection made by awc client.
    ///
    /// You can use this struct directly by passing the `Framed` object you get from `awc` into this constructor.
    /// This will create a background worker in actix system (on current thread), which will encode and decode STOMP messages for you.
    /// It also manages websocket ping-pong heartbeat.
    pub fn from_framed(ws_framed: Framed<BoxedSocket, Codec>) -> Self {
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
