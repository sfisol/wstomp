use std::{sync::Arc};

use actix_http::Uri;
use log::info;
use awc::{Client, error::HttpError};
use rustls::ClientConfig;
use tokio_rustls::rustls::{RootCertStore, pki_types::{CertificateDer, PrivateKeyDer, TrustAnchor}};
use crate::{WStompClient, WStompConfig, WStompConnectError};

/// Connect to STOMP server through SSL
pub async fn connect_ssl<U>(url: U) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url).ssl().build_and_connect().await
}

/// Connect to STOMP server through SSL using authorization token.
///
/// Creates and builds the client automatically.
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

/// Connect to STOMP server through SSL using password.
///
/// Creates and builds the client automatically.
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

/// Connect to STOMP server through SSL using certificate.
///
/// Creates and builds the client automatically.
pub async fn connect_ssl_with_cert<U>(
    url: U,
    cert_chain: Vec<CertificateDer<'static>>,
    key_der: PrivateKeyDer<'static>,
    ca_certs: Vec<CertificateDer<'static>>
) -> Result<WStompClient, WStompConnectError>
where
    Uri: TryFrom<U>,
    <Uri as TryFrom<U>>::Error: Into<HttpError>,
{
    WStompConfig::new(url)
        .ssl()
        .cert(cert_chain)
        .key(key_der)
        .ca_certs(ca_certs)
        .build_and_connect()
        .await
}

// This creates ssl client which forces usage of http/1.1 for compatibility with various SockJS servers
pub(crate) fn create_ssl_client(
    cert_chain: Option<Vec<CertificateDer<'static>>>,
    key_der: Option<Arc<PrivateKeyDer<'static>>>,
    ca_certs: Option<Vec<CertificateDer<'static>>>
) -> Client {
    // 1. Create a root certificate store
    let mut root_store = RootCertStore::empty();
    if let Some(ca_certs) = ca_certs {
        for cert in ca_certs {
            match root_store.add(cert) {
                Ok(()) => info!("Successfully added CA certificate."),
                Err(error) => panic!("Error adding CA certificate: {error}")
            }
                // .map_err(|e| anyhow!("Failed to add CA certificate: {}", e));
        }
    } else {
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned().map(|ta| 
            TrustAnchor {
                subject: ta.subject,
                subject_public_key_info: ta.subject_public_key_info,
                name_constraints: ta.name_constraints  
            })
        );
    }

    // 2. Create a rustls ClientConfig
    let config_builder = ClientConfig::builder()
        .with_root_certificates(root_store);

    let mut config = if let (Some(cert), Some(key)) = (cert_chain, key_der) {
        match config_builder.with_client_auth_cert(cert, key.clone_key()) {
            Ok(config) => config,
            Err(error) => {
                panic!("Error initializing TLS client certificate authentication. {error:?}");
            }
        }
    } else {
        config_builder.with_no_client_auth()
    };

    // // 3. IMPORTANT: Force HTTP/1.1 for ALPN
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    // // 4. Create an awc Connector with the custom rustls config
    let connector = awc::Connector::new().rustls_0_23(Arc::new(config));

    Client::builder().connector(connector).finish()
}