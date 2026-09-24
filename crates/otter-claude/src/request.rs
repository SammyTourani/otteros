use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use otter_json::{Object, Value, to_string};
use otter_http::{Request, Url};

/// A content block in a message (assistant or user).
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Plain text content
    Text(String),
    /// Thinking block with optional signature
    Thinking {
        /// The thinking content
        content: String,
        /// Optional signature from the API
        signature: Option<String>,
    },
    /// Tool use block
    ToolUse {
        /// Tool invocation ID
        id: String,
        /// Tool name
        name: String,
        /// Tool input as JSON string
        input: String,
    },
    /// Fallback block (recording a model switch)
    Fallback {
        /// The fallback reason or model
        reason: String,
    },
}

/// A message in the conversation.
#[derive(Debug, Clone)]
pub struct Message {
    /// "user" or "assistant"
    pub role: String,
    /// Content blocks for this message
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Creates a new user message with the given text.
    pub fn user(text: impl Into<String>) -> Self {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    /// Creates a new assistant message.
    pub fn assistant() -> Self {
        Message {
            role: "assistant".into(),
            content: Vec::new(),
        }
    }

    /// Adds a content block to this message.
    pub fn with_content(mut self, block: ContentBlock) -> Self {
        self.content.push(block);
        self
    }
}

/// A tool definition for the API.
#[derive(Debug, Clone)]
pub struct Tool {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool input schema as a JSON object
    pub input_schema: Object,
}

impl Tool {
    /// Creates a new tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Object,
    ) -> Self {
        Tool {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// A conversation state for the Claude API.
#[derive(Debug, Clone)]
pub struct Conversation {
    /// System prompt
    pub system: String,
    /// Available tools
    pub tools: Vec<Tool>,
    /// Message history
    pub messages: Vec<Message>,
}

impl Conversation {
    /// Creates a new conversation with a system prompt.
    pub fn new(system: impl Into<String>) -> Self {
        Conversation {
            system: system.into(),
            tools: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Adds a tool to the conversation.
    pub fn add_tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }

    /// Adds a message to the conversation.
    pub fn add_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }
}

/// Builds a POST request to the Claude Messages API.
#[allow(dead_code)]
pub fn build_request(
    url: Url,
    api_key: &str,
    conversation: &Conversation,
    model: &str,
    max_tokens: u32,
    fallbacks_enabled: bool,
) -> Result<Request, crate::ValidationError> {
    let mut body = Object::new();

    // Required fields
    body.insert("model".into(), Value::string(model));
    body.insert("max_tokens".into(), Value::Number((max_tokens as i64).into()));
    body.insert("stream".into(), Value::Bool(true));

    // Fallbacks
    if fallbacks_enabled {
        body.insert("fallbacks".into(), Value::string("default"));
    }

    // Thinking config
    let mut thinking = Object::new();
    thinking.insert("type".into(), Value::string("adaptive"));
    thinking.insert("display".into(), Value::string("summarized"));
    body.insert("thinking".into(), Value::Object(thinking));

    // Output config
    let mut output_config = Object::new();
    output_config.insert("effort".into(), Value::string("high"));
    body.insert("output_config".into(), Value::Object(output_config));

    // System prompt
    body.insert("system".into(), Value::string(&conversation.system));

    // Tools (eager_input_streaming: true)
    if !conversation.tools.is_empty() {
        let mut tools_array = Vec::new();
        for tool in &conversation.tools {
            let mut tool_obj = Object::new();
            tool_obj.insert("name".into(), Value::string(&tool.name));
            tool_obj.insert("description".into(), Value::string(&tool.description));
            tool_obj.insert("input_schema".into(), Value::Object(tool.input_schema.clone()));
            tool_obj.insert("eager_input_streaming".into(), Value::Bool(true));
            tools_array.push(Value::Object(tool_obj));
        }
        body.insert("tools".into(), Value::Array(tools_array));
    }

    // Messages
    let mut messages_array = Vec::new();
    for message in &conversation.messages {
        let mut msg_obj = Object::new();
        msg_obj.insert("role".into(), Value::string(&message.role));

        // Build content array
        let mut content_array = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => {
                    let mut text_block = Object::new();
                    text_block.insert("type".into(), Value::string("text"));
                    text_block.insert("text".into(), Value::string(text));
                    content_array.push(Value::Object(text_block));
                }
                ContentBlock::Thinking { content, signature } => {
                    let mut thinking_block = Object::new();
                    thinking_block.insert("type".into(), Value::string("thinking"));
                    thinking_block.insert("thinking".into(), Value::string(content));
                    if let Some(sig) = signature {
                        thinking_block.insert("signature".into(), Value::string(sig));
                    }
                    content_array.push(Value::Object(thinking_block));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let mut tool_block = Object::new();
                    tool_block.insert("type".into(), Value::string("tool_use"));
                    tool_block.insert("id".into(), Value::string(id));
                    tool_block.insert("name".into(), Value::string(name));
                    // Parse input as JSON to maintain structure
                    if let Ok(input_value) = otter_json::parse_str(input) {
                        tool_block.insert("input".into(), input_value);
                    } else {
                        // If parsing fails, store as string
                        tool_block.insert("input".into(), Value::string(input));
                    }
                    content_array.push(Value::Object(tool_block));
                }
                ContentBlock::Fallback { reason } => {
                    let mut fallback_block = Object::new();
                    fallback_block.insert("type".into(), Value::string("fallback"));
                    fallback_block.insert("reason".into(), Value::string(reason));
                    content_array.push(Value::Object(fallback_block));
                }
            }
        }
        msg_obj.insert("content".into(), Value::Array(content_array));

        // Handle tool_result if this is a user message with tool results
        if message.role == "user" {
            // Check if any content block is a tool result
            // (these should be included in the content array as-is)
        }

        messages_array.push(Value::Object(msg_obj));
    }
    body.insert("messages".into(), Value::Array(messages_array));

    // Serialize the body to JSON
    let body_json = to_string(&Value::Object(body));
    let body_bytes = body_json.into_bytes();

    // Build the HTTP request
    let mut request = Request::post(url, body_bytes);
    request = request
        .with_header("content-type", "application/json")
        .with_header("x-api-key", api_key)
        .with_header("anthropic-version", "2023-06-01")
        .with_header(
            "anthropic-beta",
            "server-side-fallback-2026-07-01",
        );

    Ok(request)
}
