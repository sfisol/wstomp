use actix_http::Uri;
use async_stomp::client::Connector;
use awc::{
    error::{HttpError, WsClientError},
    ws::WebsocketsRequest,
};
use backoff::{ExponentialBackoffBuilder, backoff::Backoff};
use std::time::Duration;
use tokio::time::sleep;

use crate::{WStompClient, WStompConfig, WStompConnectError, config::WStompConfigOpts};

/// Connect to STOMP server without additional parameters
///
/// Creates and builds the client automatically.
pub async fn connect<U>(url: U) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url).build_and_connect().await
}

/// Connect to STOMP server using authorization token
///
/// Creates and builds the client automatically.
pub async fn connect_with_token<U>(
    url: U,
    auth_token: impl Into<String>,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url)
        .auth_token(auth_token)
        .build_and_connect()
        .await
}

/// Connect to STOMP server using password
///
/// Creates and builds the client automatically.
pub async fn connect_with_pass<U>(
    url: U,
    login: impl Into<String>,
    passcode: impl Into<String>,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url)
        .login(login)
        .passcode(passcode)
        .build_and_connect()
        .await
}

pub trait StompConnect {
    /// Complete request construction and connect to a WebSocket server, returning a StompClient.
    ///
    /// Does not send CONNECT message to STOMP server.
    fn stomp_connect(self) -> impl Future<Output = Result<WStompClient, WStompConnectError>>;
}

impl StompConnect for WebsocketsRequest {
    async fn stomp_connect(self) -> Result<WStompClient, WStompConnectError> {
        let (_response, framed_connection) = self
            .connect()
            .await
            .map_err(WStompConnectError::WsClientError)?;

        Ok(WStompClient::from_framed(framed_connection))
    }
}

impl<U> WStompConfig<U>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    /// Build the client and connect (once).
    pub async fn build_and_connect(self) -> Result<WStompClient, WStompConnectError> {
        let (url, opts) = self.into_parts();

        let uri = Uri::try_from(url).map_err(|e| {
            let err: HttpError = e.into();
            WStompConnectError::WsClientError(WsClientError::from(err))
        })?;

        inner_connect(uri, opts).await
    }

    /// Build the client and spawns connect procedure with reconnection mechanism.
    /// The result from the connection procedure and all subsequent reconnection attempts is passed into the callback.
    pub fn build_and_connect_with_reconnection_cb<F, R>(
        self,
        cb: F,
    ) -> Result<(), WStompConnectError>
    where
        F: Fn(Result<WStompClient, WStompConnectError>) -> R + 'static,
        R: Future<Output = ()>,
    {
        let (url, opts) = self.into_parts();

        let uri = Uri::try_from(url).map_err(|e| {
            let err: HttpError = e.into();
            WStompConnectError::WsClientError(WsClientError::from(err))
        })?;

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_secs(opts.retry_initial_interval))
            .with_max_interval(Duration::from_secs(opts.retry_max_interval))
            .with_multiplier(opts.retry_multiplier)
            .with_max_elapsed_time(opts.retry_max_elapsed_time.map(Duration::from_secs))
            .build();

        actix_rt::spawn(async move {
            loop {
                let tx = inner_connect(uri.clone(), opts.clone()).await;

                if tx.is_ok() {
                    backoff.reset();
                } else if let Some(duration) = backoff.next_backoff() {
                    sleep(duration).await;
                } else {
                    cb(Err(WStompConnectError::ReconnectionLimit)).await;
                    break;
                }

                cb(tx).await;
            }
        });

        Ok(())
    }
}

pub(crate) fn headers_for_token(auth_token: impl Into<String>) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), auth_token.into())]
}

async fn inner_connect(
    uri: Uri,
    opts: WStompConfigOpts,
) -> Result<WStompClient, WStompConnectError> {
    let client = if let Some(client) = opts.client {
        client
    } else {
        #[cfg(feature = "rustls")]
        if opts.ssl {
            crate::connect_ssl::create_ssl_client(opts.cert_chain, opts.key_der, opts.ca_certs)
        } else {
            awc::Client::default()
        }
        #[cfg(not(feature = "rustls"))]
        awc::Client::default()
    };

    let (authority, host_name) = uri
        .authority()
        .map(|a| (a.to_string(), a.host().to_string()))
        .unwrap_or_default();

    let mut headers = opts.additional_headers;

    if let Some(auth_token) = opts.auth_token {
        headers.extend(headers_for_token(auth_token));
    }

    let stomp_client = client.ws::<Uri>(uri).stomp_connect().await?;

    let connect_msg = Connector::builder()
        .server(authority.clone())
        .virtualhost(authority)
        .headers(headers)
        .use_tls(true)
        .tls_server_name(host_name);

    let connect_msg = if let Some(login) = opts.login
        && let Some(passcode) = opts.passcode
    {
        connect_msg.login(login).passcode(passcode).msg()
    } else {
        connect_msg.msg()
    };

    stomp_client
        .send(connect_msg)
        .await
        .map_err(Box::new)
        .map_err(WStompConnectError::ConnectMessageFailed)?;

    Ok(stomp_client)
}
