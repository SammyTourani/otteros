#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

mod transport;
mod host;

pub use transport::AgentTransport;
pub use host::ToolHostAdapter;

use otter_claude::{Conversation, Tool, ClientConfig, ApiKey, run_tool_loop, LoopError, LoopOutcome, ContentBlock};
use otter_tools::Session;
use otter_json::Value;

/// A byte stream for sending and receiving data.
pub trait ByteStream {
    /// Read up to `buf.len()` bytes from the stream.
    /// Returns the number of bytes read; Ok(0) means EOF.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String>;

    /// Write all the data to the stream.
    fn write_all(&mut self, data: &[u8]) -> Result<(), String>;
}

/// A connector for establishing TCP connections.
pub trait Connector {
    /// Connect to a remote host and port.
    fn connect(&mut self, host: &str, port: u16) -> Result<Box<dyn ByteStream>, String>;
}

/// Configuration for the agent backend.
#[derive(Clone)]
pub struct Backend {
    /// The hostname to connect to.
    pub host: String,
    /// The port to connect to.
    pub port: u16,
    /// The server name for TLS SNI.
    pub server_name: String,
    /// The API path (e.g. "/v1/messages").
    pub path: String,
    /// The API key for authentication.
    pub api_key: String,
    /// The root CA certificates in DER format.
    pub roots_der: Vec<Vec<u8>>,
    /// The current Unix timestamp.
    pub now_unix: u64,
}

/// Errors that can occur when operating the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// Failed to connect to the backend.
    Connect(String),
    /// TLS error.
    Tls(String),
    /// HTTP error with status code and body.
    Http(u16, String),
    /// API error.
    Api(String),
    /// Protocol error (e.g., stream ended prematurely).
    Protocol(String),
    /// Too many turns in the conversation.
    TooManyTurns,
}

impl core::fmt::Display for AgentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AgentError::Connect(msg) => write!(f, "Connect: {}", msg),
            AgentError::Tls(msg) => write!(f, "Tls: {}", msg),
            AgentError::Http(status, body) => write!(f, "Http {}: {}", status, body),
            AgentError::Api(msg) => write!(f, "Api: {}", msg),
            AgentError::Protocol(msg) => write!(f, "Protocol: {}", msg),
            AgentError::TooManyTurns => write!(f, "TooManyTurns"),
        }
    }
}

/// The agent: sends Messages API requests and manages a conversation.
pub struct Agent<O: otter_tools::Os> {
    backend: Backend,
    connector: Box<dyn Connector>,
    rng: Box<dyn otter_tls::Rng>,
    session: Session<O>,
    conversation: Conversation,
    transport_error: Rc<RefCell<Option<AgentError>>>,
}

impl<O: otter_tools::Os> Agent<O> {
    /// Create a new agent.
    pub fn new(
        backend: Backend,
        connector: Box<dyn Connector>,
        rng: Box<dyn otter_tls::Rng>,
        session: Session<O>,
        system_prompt: &str,
    ) -> Self {
        let mut conversation = Conversation::new(system_prompt);

        // Add the seven tools from otter-tools::TOOLS
        for spec in otter_tools::TOOLS.iter() {
            // Parse the input schema JSON
            let input_schema = match otter_json::parse_str(spec.input_schema) {
                Ok(Value::Object(obj)) => obj,
                _ => otter_json::Object::new(), // Fallback to empty object
            };

            let tool = Tool {
                name: spec.name.to_string(),
                description: spec.description.to_string(),
                input_schema,
            };
            conversation = conversation.add_tool(tool);
        }

        Agent {
            backend,
            connector,
            rng,
            session,
            conversation,
            transport_error: Rc::new(RefCell::new(None)),
        }
    }

    /// Ask the agent a question and get a response.
    pub fn ask(&mut self, user_text: &str) -> Result<String, AgentError> {
        // Add the user message to the conversation
        let msg = otter_claude::Message {
            role: "user".into(),
            content: alloc::vec![ContentBlock::Text(user_text.to_string())],
        };
        self.conversation.push_message(msg);

        // Clear any previous transport error
        *self.transport_error.borrow_mut() = None;

        // Create a transport with shared error storage
        let mut transport = AgentTransport::new(
            self.backend.clone(),
            &mut *self.connector,
            &mut *self.rng,
            self.transport_error.clone(),
        );

        // Create a tool host adapter
        let mut host = ToolHostAdapter::new(&mut self.session);

        // Run the tool loop
        let config = ClientConfig::new(ApiKey::new(&self.backend.api_key))
            .with_model("claude-opus-5")
            .with_max_tokens(4096)
            .with_fallbacks(true);

        let result = run_tool_loop(&config, &mut self.conversation, &mut transport, &mut host, 16);

        // Check if there's a stored transport error (e.g., from TLS cert validation)
        if let Some(stored_error) = self.transport_error.borrow_mut().take() {
            return Err(stored_error);
        }

        match result {
            Ok(LoopOutcome::Done { text }) => Ok(text),
            Ok(LoopOutcome::Refused) => Err(AgentError::Api("Refusal".into())),
            Err(LoopError::Api(msg)) => Err(AgentError::Api(msg)),
            Err(LoopError::Transport(msg)) => {
                // Parse error prefixes to determine the actual error type
                if let Some(rest) = msg.strip_prefix("connect:") {
                    Err(AgentError::Connect(rest.to_string()))
                } else if let Some(rest) = msg.strip_prefix("tls:") {
                    Err(AgentError::Tls(rest.to_string()))
                } else if let Some(rest) = msg.strip_prefix("http:") {
                    // Try to parse as HTTP status code
                    if let Ok(status) = rest.parse::<u16>() {
                        Err(AgentError::Http(status, "".into()))
                    } else {
                        Err(AgentError::Http(0, rest.to_string()))
                    }
                } else if let Some(rest) = msg.strip_prefix("api:") {
                    Err(AgentError::Api(rest.to_string()))
                } else {
                    Err(AgentError::Protocol(msg))
                }
            }
            Err(LoopError::MaxTokens) => Err(AgentError::Api("Max tokens reached".into())),
            Err(LoopError::TooManyTurns) => Err(AgentError::TooManyTurns),
        }
    }

    /// Get access to the tools session.
    pub fn tools(&self) -> &Session<O> {
        &self.session
    }
}
