use std::{future::Future, pin::Pin, sync::Arc};

#[cfg(feature = "rustls")]
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Closure type for dynamic auth token retrieval. Invoked on every
/// (re)connection attempt so the token can be refreshed out-of-band by the
/// application (e.g. via a background token-refresh task) without recreating
/// the wstomp client.
pub type AuthTokenFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// Marker for a [`WStompConfig`] without a reconnection callback. In this
/// state [`WStompConfig::build_and_connect`] is a one-shot async method that
/// resolves to a [`crate::WStompClient`].
pub struct NoReconnect;

/// Marker for a [`WStompConfig`] with a reconnection callback installed via
/// [`WStompConfig::on_reconnect`]. In this state
/// [`WStompConfig::build_and_connect`] spawns the reconnect loop and returns
/// a [`crate::WStompReconnectHandle`].
pub struct Reconnecting<F>(pub(crate) F);

pub struct WStompConfig<U, R = NoReconnect> {
    pub(crate) url: U,
    pub(crate) opts: WStompConfigOpts,
    pub(crate) reconnect: R,
}

#[derive(Clone)]
pub struct WStompConfigOpts {
    pub ssl: bool,
    pub auth_token: Option<AuthTokenFn>,
    pub login: Option<String>,
    pub passcode: Option<String>,
    #[cfg(feature = "rustls")]
    pub cert_chain: Option<Vec<CertificateDer<'static>>>,
    #[cfg(feature = "rustls")]
    pub key_der: Option<Arc<PrivateKeyDer<'static>>>,
    #[cfg(feature = "rustls")]
    pub ca_certs: Option<Vec<CertificateDer<'static>>>,
    pub additional_headers: Vec<(String, String)>,
    pub client: Option<awc::Client>,

    // Reconnection opts in seconds
    pub retry_initial_interval: u64,
    pub retry_max_interval: u64,
    pub retry_multiplier: f64,
    pub retry_max_elapsed_time: Option<u64>,
}

impl Default for WStompConfigOpts {
    fn default() -> Self {
        Self {
            ssl: Default::default(),
            auth_token: Default::default(),
            login: Default::default(),
            passcode: Default::default(),
            #[cfg(feature = "rustls")]
            cert_chain: Default::default(),
            #[cfg(feature = "rustls")]
            key_der: Default::default(),
            #[cfg(feature = "rustls")]
            ca_certs: Default::default(),
            additional_headers: Default::default(),
            client: Default::default(),

            retry_initial_interval: 3,
            retry_max_interval: 60,
            retry_multiplier: 1.2,
            retry_max_elapsed_time: None,
        }
    }
}

impl<U> WStompConfig<U, NoReconnect> {
    pub fn new(url: U) -> Self {
        Self {
            url,
            opts: WStompConfigOpts::default(),
            reconnect: NoReconnect,
        }
    }
}

impl<U, R> WStompConfig<U, R> {
    /// Get url to which this config is assigned to use.
    pub fn get_url(&self) -> &U {
        &self.url
    }

    /// Get options for this config.
    pub fn get_opts(&self) -> &WStompConfigOpts {
        &self.opts
    }

    /// De-couple url and options in this config.
    pub fn into_parts(self) -> (U, WStompConfigOpts) {
        (self.url, self.opts)
    }

    // Setters

    /// Enables TLS/SSL encryption for the connection.
    ///
    /// When set, the client will attempt to perform a secure handshake
    /// (typically for `wss://` schemes).
    pub fn ssl(mut self) -> Self {
        self.opts.ssl = true;
        self
    }

    /// Sets a static authentication token for the connection. The token is
    /// fixed for the lifetime of the client — use [Self::auth_token_fn] when
    /// the token needs to be refreshed between reconnects.
    pub fn auth_token(mut self, auth_token: impl Into<String>) -> Self {
        let token = auth_token.into();
        self.opts.auth_token = Some(Arc::new(move || {
            let token = token.clone();
            Box::pin(async move { Some(token) })
        }));
        self
    }

    /// Sets a dynamic authentication token provider. `f` is invoked on every
    /// (re)connection attempt, so returning a fresh token allows the app to
    /// refresh credentials without tearing down the wstomp client.
    ///
    /// Returning `None` from `f` omits the `Authorization` header entirely.
    pub fn auth_token_fn<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<String>> + Send + 'static,
    {
        self.opts.auth_token = Some(Arc::new(move || Box::pin(f())));
        self
    }

    /// Sets the `login` header for STOMP authentication.
    pub fn login(mut self, login: impl Into<String>) -> Self {
        self.opts.login = Some(login.into());
        self
    }

    /// Sets the `passcode` header for STOMP authentication.
    pub fn passcode(mut self, passcode: impl Into<String>) -> Self {
        self.opts.passcode = Some(passcode.into());
        self
    }

    /// Configures the TLS connection to authenticate via certificate.
    #[cfg(feature = "rustls")]
    pub fn cert(mut self, cert_chain: impl Into<Vec<CertificateDer<'static>>>) -> Self {
        self.opts.cert_chain = Some(cert_chain.into());
        self
    }

    #[cfg(feature = "rustls")]
    pub fn key(mut self, key: impl Into<Arc<PrivateKeyDer<'static>>>) -> Self {
        self.opts.key_der = Some(key.into());
        self
    }

    #[cfg(feature = "rustls")]
    pub fn ca_certs(mut self, ca_certs: impl Into<Vec<CertificateDer<'static>>>) -> Self {
        self.opts.ca_certs = Some(ca_certs.into());
        self
    }

    /// Appends a list of custom headers to the connection configuration.
    ///
    /// These headers will be included in the STOMP `CONNECT` frame.
    /// This method does not replace existing headers; it extends the list.
    pub fn add_headers(mut self, additional_headers: Vec<(String, String)>) -> Self {
        self.opts.additional_headers.extend(additional_headers);
        self
    }

    /// Sets a custom `awc::Client` instance.
    ///
    /// Use this if you need to provide a pre-configured HTTP client (e.g.,
    /// with custom timeouts, proxy settings, or connector configurations)
    /// instead of letting the library create a default one.
    pub fn client(mut self, client: awc::Client) -> Self {
        self.opts.client = Some(client);
        self
    }

    /// When [`Self::on_reconnect`] is used to install a reconnection
    /// callback, sets the initial retry interval in seconds.
    ///
    /// Example: Start retrying after 3 seconds.
    pub fn retry_initial_interval(mut self, seconds: u64) -> Self {
        self.opts.retry_initial_interval = seconds;
        self
    }

    /// When [`Self::on_reconnect`] is used to install a reconnection
    /// callback, sets the maximum retry interval in seconds.
    ///
    /// Example: Cap the wait time at 30 seconds.
    pub fn retry_max_interval(mut self, seconds: u64) -> Self {
        self.opts.retry_max_interval = seconds;
        self
    }

    /// When [`Self::on_reconnect`] is used to install a reconnection
    /// callback, sets the multiplier for the backoff.
    ///
    /// Example: 2.0 doubles the wait time after every failure.
    pub fn retry_multiplier(mut self, multiplier: f64) -> Self {
        self.opts.retry_multiplier = multiplier;
        self
    }

    /// When [`Self::on_reconnect`] is used to install a reconnection
    /// callback, sets a maximum total time to try reconnecting before giving
    /// up.
    ///
    /// Defaults to no limit if method not invoked.
    pub fn retry_max_elapsed_time(mut self, seconds: u64) -> Self {
        self.opts.retry_max_elapsed_time = Some(seconds);
        self
    }
}
