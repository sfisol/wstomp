use std::{pin::Pin, time::Duration};

use actix_http::Uri;
use async_stomp::client::Connector;
use awc::{
    error::{HttpError, WsClientError},
    ws::WebsocketsRequest,
};
use futures_util::Stream;
use tokio::time::sleep;

use crate::{WStompClient, WStompConfig, WStompConnectError, config::WStompConfigOpts};

/// Connect to STOMP server without additional parameters
pub async fn connect<U>(url: U) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url).build_and_connect().await
}

/// Connect to STOMP server using authorization token
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
    pub async fn build_and_connect(self) -> Result<WStompClient, WStompConnectError> {
        let (url, opts) = self.into_inner();

        let client = if let Some(client) = opts.client {
            client
        } else {
            #[cfg(feature = "rustls")]
            if opts.ssl {
                crate::connect_ssl::create_ssl_client()
            } else {
                awc::Client::default()
            }
            #[cfg(not(feature = "rustls"))]
            awc::Client::default()
        };

        let uri = Uri::try_from(url).map_err(|e| {
            let err: HttpError = e.into();
            WStompConnectError::WsClientError(WsClientError::from(err))
        })?;

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
            .map_err(WStompConnectError::ConnectMessageFailed)?;

        Ok(stomp_client)
    }

    pub fn build_and_connect_with_reconnection_cb<F: Fn(Result<WStompClient, WStompConnectError>) -> Pin<Box<dyn Future<Output = ()>>> + 'static> (
        self,
        cb: F
    ) {
        let (url, opts) = self.into_inner();

        let uri = Uri::try_from(url).map_err(|e| {
            let err: HttpError = e.into();
            WStompConnectError::WsClientError(WsClientError::from(err))
        }).unwrap(); // TODO

        actix_rt::spawn(async move {
            loop {
                let tx = inner_connect(uri.clone(), opts.clone()).await;

                cb(tx).await;

                sleep(Duration::from_secs(3)).await;
            }
        });
    }
}

pub(crate) fn headers_for_token(auth_token: impl Into<String>) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), auth_token.into())]
}

async fn inner_connect(uri: Uri, opts: WStompConfigOpts) -> Result<WStompClient, WStompConnectError> {
    let client = if let Some(client) = opts.client {
        client
    } else {
        #[cfg(feature = "rustls")]
        if opts.ssl {
            crate::connect_ssl::create_ssl_client()
        } else {
            awc::Client::default()
        }
        #[cfg(not(feature = "rustls"))]
        awc::Client::default()
    };

    // let uri = Uri::try_from(url).map_err(|e| {
    //     let err: HttpError = e.into();
    //     WStompConnectError::WsClientError(WsClientError::from(err))
    // })?;

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
        .map_err(WStompConnectError::ConnectMessageFailed)?;

    Ok(stomp_client)
}
