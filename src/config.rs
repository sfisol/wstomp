#[cfg(feature = "rustls")]
use tokio_rustls::rustls::Certificate;
use tokio_rustls::rustls::{PrivateKey};

pub struct WStompConfig<U> {
    url: U,
    opts: WStompConfigOpts,
}

#[derive(Clone)]
pub struct WStompConfigOpts {
    #[cfg(feature = "rustls")]
    pub ssl: bool,
    pub auth_token: Option<String>,
    pub login: Option<String>,
    pub passcode: Option<String>,
    pub cert_chain: Option<Vec<Certificate>>,
    pub key_der: Option<PrivateKey>,
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
            cert_chain: Default::default(),
            key_der: Default::default(),
            additional_headers: Default::default(),
            client: Default::default(),

            retry_initial_interval: 3,
            retry_max_interval: 60,
            retry_multiplier: 1.2,
            retry_max_elapsed_time: None,
        }
    }
}

impl<U> WStompConfig<U> {
    pub fn new(url: U) -> Self {
        Self {
            url,
            opts: WStompConfigOpts::default(),
        }
    }

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

    /// Sets the authentication token for the connection.
    pub fn auth_token(mut self, auth_token: impl Into<String>) -> Self {
        self.opts.auth_token = Some(auth_token.into());
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
    pub fn cert(mut self, cert_chain: impl Into<Vec<Certificate>>) -> Self {
        self.opts.cert_chain = Some(cert_chain.into());
        self
    }

    pub fn key(mut self, key_der: impl Into<PrivateKey>) -> Self {
        self.opts.key_der = Some(key_der.into());
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

    /// If [Self::build_and_connect_with_reconnection_cb] method is used,
    /// sets the initial retry interval in seconds.
    ///
    /// Example: Start retrying after 3 seconds.
    pub fn retry_initial_interval(mut self, seconds: u64) -> Self {
        self.opts.retry_initial_interval = seconds;
        self
    }

    /// If [Self::build_and_connect_with_reconnection_cb] method is used,
    /// sets the maximum retry interval in seconds.
    ///
    /// Example: Cap the wait time at 30 seconds.
    pub fn retry_max_interval(mut self, seconds: u64) -> Self {
        self.opts.retry_max_interval = seconds;
        self
    }

    /// If [Self::build_and_connect_with_reconnection_cb] method is used,
    /// sets the multiplier for the backoff.
    ///
    /// Example: 2.0 doubles the wait time after every failure.
    pub fn retry_multiplier(mut self, multiplier: f64) -> Self {
        self.opts.retry_multiplier = multiplier;
        self
    }

    /// If [Self::build_and_connect_with_reconnection_cb] method is used,
    /// sets a maximum total time to try reconnecting before giving up.
    ///
    /// Defaults to no limit if method not invoked.
    pub fn retry_max_elapsed_time(mut self, seconds: u64) -> Self {
        self.opts.retry_max_elapsed_time = Some(seconds);
        self
    }
}
