use actix_http::Uri;
use awc::{Client, error::HttpError};
use std::sync::Arc;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};

use crate::{WStompClient, WStompConfig, WStompConnectError};

/// Connect to STOMP server through SSL
pub async fn connect_ssl<U>(url: U) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url).ssl().build_and_connect().await
}

/// Connect to STOMP server through SSL using authorization token
pub async fn connect_ssl_with_token<U>(
    url: U,
    auth_token: impl Into<String>,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url)
        .ssl()
        .auth_token(auth_token)
        .build_and_connect()
        .await
}

/// Connect to STOMP server through SSL using password
pub async fn connect_ssl_with_pass<U>(
    url: U,
    login: String,
    passcode: String,
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url)
        .ssl()
        .login(login)
        .passcode(passcode)
        .build_and_connect()
        .await
}

// This creates ssl client which forces usage of http/1.1 for compatibility with various SockJS servers
pub(crate) fn create_ssl_client() -> Client {
    // 1. Create a root certificate store

    // Switch to this after updating rustls
    // let root_store = rustls::RootCertStore {
    //     roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    // };

    let mut root_store = RootCertStore::empty();
    root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject.as_ref(),
            ta.subject_public_key_info.as_ref(),
            ta.name_constraints.as_deref(),
        )
    }));

    // 2. Create a rustls ClientConfig
    let mut config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // // 3. IMPORTANT: Force HTTP/1.1 for ALPN
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    // // 4. Create an awc Connector with the custom rustls config
    let connector = awc::Connector::new().rustls(Arc::new(config));

    Client::builder().connector(connector).finish()
}
