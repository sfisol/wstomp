use actix_http::Uri;
use async_stomp::client::Connector;
use awc::{
    error::{HttpError, WsClientError},
    ws::WebsocketsRequest,
};
use backoff::{ExponentialBackoffBuilder, backoff::Backoff};
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::{
    WStompClient, WStompConfig, WStompConnectError,
    config::{NoReconnect, Reconnecting, WStompConfigOpts},
};

/// Signal returned by the reconnection callback to control what the
/// reconnection loop does next.
#[derive(Debug, Clone)]
pub enum ReconnectControl {
    /// Reconnect using the normal exponential-backoff schedule. Short-lived
    /// sessions (closed within one `retry_initial_interval`) are treated as
    /// failures for backoff purposes so a subscribe error doesn't cause a
    /// tight reconnect loop.
    Continue,
    /// Sleep for the given duration before the next reconnect attempt, then
    /// reset the backoff. Use this when the application already knows how
    /// long the server needs to cool down (e.g. rate-limit headers).
    DelayThen(Duration),
    /// Stop the reconnection loop cleanly.
    Stop,
}

/// Handle to a running reconnection loop. Dropping it aborts the loop, as
/// does calling [`Self::abort`] explicitly.
pub struct WStompReconnectHandle {
    join: actix_rt::task::JoinHandle<()>,
}

impl WStompReconnectHandle {
    /// Abort the reconnection loop. Safe to call more than once; subsequent
    /// calls are no-ops.
    pub fn abort(&self) {
        self.join.abort();
    }

    /// Returns whether the reconnection loop has finished (either aborted or
    /// ran to completion via [`ReconnectControl::Stop`]).
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

impl Drop for WStompReconnectHandle {
    fn drop(&mut self) {
        self.join.abort();
    }
}

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

impl<U> WStompConfig<U, NoReconnect> {
    /// Install a reconnection callback. Switches the config into the
    /// reconnecting state, where [`Self::build_and_connect`] spawns a
    /// reconnect loop and hands the resulting client (or connection error)
    /// to `cb` on every attempt. The callback's return value
    /// ([`ReconnectControl`]) drives the loop.
    pub fn on_reconnect<F, R>(self, cb: F) -> WStompConfig<U, Reconnecting<F>>
    where
        F: Fn(Result<WStompClient, WStompConnectError>) -> R + 'static,
        R: Future<Output = ReconnectControl>,
    {
        let (url, opts) = self.into_parts();
        WStompConfig {
            url,
            opts,
            reconnect: Reconnecting(cb),
        }
    }
}

impl<U> WStompConfig<U, NoReconnect>
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

        let auth_token = match &opts.auth_token {
            Some(f) => f().await,
            None => None,
        };
        inner_connect(uri, opts, auth_token).await
    }
}

impl<U, F, R> WStompConfig<U, Reconnecting<F>>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
    F: Fn(Result<WStompClient, WStompConnectError>) -> R + 'static,
    R: Future<Output = ReconnectControl>,
{
    /// Build the client and spawn the reconnection loop. The returned
    /// [`WStompReconnectHandle`] aborts the loop on drop.
    ///
    /// The callback installed via [`WStompConfig::on_reconnect`] is invoked
    /// after every connection attempt with either the newly connected
    /// `WStompClient` or the error from the failed attempt. Its return value
    /// ([`ReconnectControl`]) controls what the loop does next.
    pub fn build_and_connect(self) -> Result<WStompReconnectHandle, WStompConnectError> {
        let WStompConfig {
            url,
            opts,
            reconnect: Reconnecting(cb),
        } = self;

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

        // A session that closes faster than retry_initial_interval is treated
        // as a failure for backoff purposes — prevents tight reconnect loops
        // when e.g. the STOMP subscribe fails right after CONNECT succeeds.
        let min_session_duration = Duration::from_secs(opts.retry_initial_interval);

        let join = actix_rt::spawn(async move {
            loop {
                let auth_token = match &opts.auth_token {
                    Some(f) => f().await,
                    None => None,
                };

                let connect_start = Instant::now();
                let tx = inner_connect(uri.clone(), opts.clone(), auth_token).await;
                let was_ok = tx.is_ok();

                let control = cb(tx).await;
                let session_duration = connect_start.elapsed();

                match control {
                    ReconnectControl::Stop => break,
                    ReconnectControl::DelayThen(d) => {
                        sleep(d).await;
                        backoff.reset();
                    }
                    ReconnectControl::Continue => {
                        let short_or_failed = !was_ok || session_duration < min_session_duration;
                        if short_or_failed {
                            if let Some(duration) = backoff.next_backoff() {
                                sleep(duration).await;
                            } else {
                                let _ = cb(Err(WStompConnectError::ReconnectionLimit)).await;
                                break;
                            }
                        } else {
                            backoff.reset();
                        }
                    }
                }
            }
        });

        Ok(WStompReconnectHandle { join })
    }
}

pub(crate) fn headers_for_token(auth_token: impl Into<String>) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), auth_token.into())]
}

async fn inner_connect(
    uri: Uri,
    opts: WStompConfigOpts,
    auth_token: Option<String>,
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

    if let Some(auth_token) = auth_token {
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
