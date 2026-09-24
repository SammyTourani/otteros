use alloc::string::String;
use core::fmt;

/// An opaque API key that prints as `<redacted>` for safety.
#[derive(Clone)]
#[allow(dead_code)]
pub struct ApiKey {
    // Stored as a String internally, but never exposed in Debug or Display.
    secret: String,
}

impl ApiKey {
    /// Creates a new API key from a string. The string is stored but never leaked.
    pub fn new(key: impl Into<String>) -> Self {
        ApiKey {
            secret: key.into(),
        }
    }

    /// Returns the actual key as a string reference.
    /// Used internally during request building, never for logging.
    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &str {
        &self.secret
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Configuration for the Claude client.
#[derive(Debug)]
pub struct ClientConfig {
    /// API key for authentication.
    pub api_key: ApiKey,
    /// Model name (defaults to "claude-opus-5").
    pub model: &'static str,
    /// Maximum tokens in response (defaults to 64000).
    pub max_tokens: u32,
    /// Whether to enable server-side fallbacks (defaults to true).
    pub fallbacks_enabled: bool,
}

impl ClientConfig {
    /// Creates a new client configuration with an API key.
    pub fn new(api_key: ApiKey) -> Self {
        ClientConfig {
            api_key,
            model: "claude-opus-5",
            max_tokens: 64000,
            fallbacks_enabled: true,
        }
    }

    /// Sets the model name.
    pub fn with_model(mut self, model: &'static str) -> Self {
        self.model = model;
        self
    }

    /// Sets the maximum tokens.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Sets whether to enable server-side fallbacks.
    pub fn with_fallbacks(mut self, enabled: bool) -> Self {
        self.fallbacks_enabled = enabled;
        self
    }
}

/// The Claude Messages API client.
#[allow(dead_code)]
pub struct Client {
    config: ClientConfig,
}

impl Client {
    /// Creates a new client with the given configuration.
    pub fn new(config: ClientConfig) -> Self {
        Client { config }
    }

    /// Returns the configuration.
    #[allow(dead_code)]
    pub(crate) fn config(&self) -> &ClientConfig {
        &self.config
    }
}
