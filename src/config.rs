pub struct WStompConfig<U> {
    url: U,
    opts: WStompConfigOpts,
}

#[derive(Default)]
pub struct WStompConfigOpts {
    #[cfg(feature = "rustls")]
    pub ssl: bool,
    pub auth_token: Option<String>,
    pub login: Option<String>,
    pub passcode: Option<String>,
    pub additional_headers: Vec<(String, String)>,
    pub client: Option<awc::Client>,
}

impl<U> WStompConfig<U> {
    pub fn new(url: U) -> Self {
        Self {
            url,
            opts: WStompConfigOpts::default(),
        }
    }

    pub fn get_url(&self) -> &U {
        &self.url
    }

    pub fn get_opts(&self) -> &WStompConfigOpts {
        &self.opts
    }

    pub fn into_inner(self) -> (U, WStompConfigOpts) {
        (self.url, self.opts)
    }

    // Setters

    pub fn ssl(mut self) -> Self {
        self.opts.ssl = true;
        self
    }

    pub fn auth_token(mut self, auth_token: impl Into<String>) -> Self {
        self.opts.auth_token = Some(auth_token.into());
        self
    }

    pub fn login(mut self, login: impl Into<String>) -> Self {
        self.opts.login = Some(login.into());
        self
    }

    pub fn passcode(mut self, passcode: impl Into<String>) -> Self {
        self.opts.passcode = Some(passcode.into());
        self
    }

    pub fn add_headers(mut self, additional_headers: Vec<(String, String)>) -> Self {
        self.opts.additional_headers.extend(additional_headers);
        self
    }

    pub fn client(mut self, client: awc::Client) -> Self {
        self.opts.client = Some(client);
        self
    }
}
