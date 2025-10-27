use actix_http::Uri;
use async_stomp::client::Connector;
use awc::{
    Client,
    error::{HttpError, WsClientError},
    ws::WebsocketsRequest,
};

use crate::{WStompClient, WStompConnectError};

pub async fn connect<U>(url: U) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    connect_with_options(Client::default(), url, vec![], None, None).await
}

pub async fn connect_with_token<U>(
    url: U,
    auth_token: impl Into<String>,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    connect_with_options(
        Client::default(),
        url,
        headers_for_token(auth_token),
        None,
        None,
    )
    .await
}

pub async fn connect_with_pass<U>(
    url: U,
    login: String,
    passcode: String,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    connect_with_options(Client::default(), url, vec![], Some(login), Some(passcode)).await
}

pub async fn connect_with_options<U>(
    client: Client,
    url: U,
    headers: Vec<(String, String)>,
    login: Option<String>,
    passcode: Option<String>,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    let uri = Uri::try_from(url).map_err(|e| {
        let err: HttpError = e.into();
        WStompConnectError::WsClientError(WsClientError::from(err))
    })?;

    let (authority, host_name) = uri
        .authority()
        .map(|a| (a.to_string(), a.host().to_string()))
        .unwrap_or_default();

    let stomp_client = client.ws::<Uri>(uri).stomp_connect().await?;

    let connect_msg = Connector::builder()
        .server(authority.clone())
        .virtualhost(authority)
        .headers(headers)
        .use_tls(true)
        .tls_server_name(host_name);

    let connect_msg = if let Some(login) = login
        && let Some(passcode) = passcode
    {
        connect_msg.login(login).passcode(passcode).msg()
    } else {
        connect_msg.msg()
    };

    stomp_client
        .send(connect_msg)
        .await
        .inspect_err(|err| println!("CONNECT error: {err}"))
        .map_err(WStompConnectError::ConnectMessageFailed)?;

    Ok(stomp_client)
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

        Ok(WStompClient::new(framed_connection))
    }
}

pub(crate) fn headers_for_token(auth_token: impl Into<String>) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), auth_token.into())]
}
