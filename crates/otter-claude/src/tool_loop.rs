use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use otter_http::Request;

use crate::{
    client::ClientConfig,
    request::{Conversation, ContentBlock, Message},
    response::StreamParser,
    validation::validate_tool_input,
};

/// A trait for sending HTTP requests to the Claude API.
pub trait Transport {
    /// Send a request and return chunks of response data.
    /// Each Vec<u8> is a chunk of the response.
    fn send(&mut self, request: &Request) -> Result<Vec<Vec<u8>>, String>;
}

/// A trait for executing tools and getting approval for state-changing tools.
pub trait ToolHost {
    /// Check if a tool is read-only (doesn't change state).
    fn read_only(&self, tool: &str) -> bool;

    /// Ask for approval to execute a state-changing tool.
    /// Returns true if approved, false if denied.
    fn approve(&mut self, tool: &str, input: &str) -> bool;

    /// Execute a tool and return the result.
    fn execute(&mut self, tool: &str, input: &str) -> Result<String, String>;
}

/// The outcome of a completed loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopOutcome {
    /// The loop completed with a final text message.
    Done {
        /// The text of the final assistant message.
        text: String,
    },
    /// The API refused the request.
    Refused,
}

/// Errors that can occur in the loop.
#[derive(Clone, PartialEq, Eq)]
pub enum LoopError {
    /// API error response.
    Api(String),
    /// Transport error (network or encoding).
    Transport(String),
    /// Model hit max tokens limit.
    MaxTokens,
    /// Too many turns were executed.
    TooManyTurns,
}

impl core::fmt::Display for LoopError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoopError::Api(msg) => write!(f, "API error: {}", msg),
            LoopError::Transport(msg) => write!(f, "Transport error: {}", msg),
            LoopError::MaxTokens => write!(f, "Max tokens reached"),
            LoopError::TooManyTurns => write!(f, "Too many turns"),
        }
    }
}

impl core::fmt::Debug for LoopError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoopError::Api(msg) => write!(f, "Api({:?})", msg),
            LoopError::Transport(msg) => write!(f, "Transport({:?})", msg),
            LoopError::MaxTokens => write!(f, "MaxTokens"),
            LoopError::TooManyTurns => write!(f, "TooManyTurns"),
        }
    }
}

/// Run the Claude tool loop: send requests, parse responses, execute tools, loop until done.
pub fn run_tool_loop(
    config: &ClientConfig,
    conversation: &mut Conversation,
    transport: &mut dyn Transport,
    host: &mut dyn ToolHost,
    max_turns: usize,
) -> Result<LoopOutcome, LoopError> {
    let api_url = otter_http::Url::parse("https://api.anthropic.com/v1/messages")
        .map_err(|e| LoopError::Transport(format!("Invalid URL: {:?}", e)))?;
    let mut turn_count = 0;

    loop {
        turn_count += 1;
        if turn_count > max_turns {
            return Err(LoopError::TooManyTurns);
        }

        // Build the request
        let request = crate::request::build_request(
            api_url.clone(),
            config.api_key.as_str(),
            conversation,
            config.model,
            config.max_tokens,
            config.fallbacks_enabled,
        )
        .map_err(|e| LoopError::Transport(format!("{:?}", e)))?;

        // Send the request and collect response chunks
        let chunks = transport.send(&request).map_err(LoopError::Transport)?;

        // Parse the response stream
        let mut parser = StreamParser::new();
        for chunk in chunks {
            parser.feed(&chunk);
        }

        // Collect all events and finalize
        let mut assistant_message = Message::assistant();
        let mut stop_reason = None;
        let mut api_error: Option<String> = None;

        while let Some(event) = parser.next_event() {
                match event {
                    crate::response::TurnEvent::Text { delta } => {
                        // Accumulate text in the current text block or create new one
                        if let Some(ContentBlock::Text(text)) = assistant_message.content.last_mut() {
                            text.push_str(&delta);
                        } else {
                            assistant_message = assistant_message.with_content(ContentBlock::Text(delta));
                        }
                    }
                    crate::response::TurnEvent::Thinking { delta } => {
                        // Accumulate thinking deltas
                        if let Some(ContentBlock::Thinking { content, .. }) = assistant_message.content.last_mut() {
                            content.push_str(&delta);
                        } else {
                            assistant_message = assistant_message.with_content(ContentBlock::Thinking {
                                content: delta,
                                signature: None,
                            });
                        }
                    }
                    crate::response::TurnEvent::ThinkingWithSignature { content, signature } => {
                        // Update the signature of the last thinking block
                        // (the content should already be accumulated from Thinking delta events)
                        if let Some(ContentBlock::Thinking { signature: sig, .. }) = assistant_message.content.last_mut() {
                            *sig = Some(signature);
                        } else {
                            // Fallback: create a thinking block if none exists
                            assistant_message = assistant_message.with_content(ContentBlock::Thinking {
                                content,
                                signature: Some(signature),
                            });
                        }
                    }
                    crate::response::TurnEvent::ToolUseStart { id, name } => {
                        // Start a new tool use block
                        assistant_message = assistant_message.with_content(ContentBlock::ToolUse {
                            id,
                            name,
                            input: String::new(),
                        });
                    }
                    crate::response::TurnEvent::ToolUseReady { id: _, name: _, input } => {
                        // Update the last ToolUse block with the input
                        if let Some(ContentBlock::ToolUse { input: tool_input, .. }) = assistant_message.content.last_mut() {
                            *tool_input = input;
                        } else {
                            // Fallback: shouldn't happen if ToolUseStart was emitted
                            assistant_message = assistant_message.with_content(ContentBlock::ToolUse {
                                id: String::new(),
                                name: String::new(),
                                input,
                            });
                        }
                    }
                    crate::response::TurnEvent::Stop { reason } => {
                        stop_reason = Some(reason);
                        break;
                    }
                    crate::response::TurnEvent::Error { message } => {
                        api_error = Some(message);
                        break;
                    }
                }
        }

        // Handle API errors
        if let Some(err_msg) = api_error {
            return Err(LoopError::Api(err_msg));
        }

        let stop_reason = stop_reason.ok_or(LoopError::Transport("No stop reason".into()))?;

        // Handle different stop reasons
        match stop_reason {
            crate::response::StopReason::EndTurn => {
                // Extract the final text message
                let mut final_text = String::new();
                for block in &assistant_message.content {
                    if let ContentBlock::Text(text) = block {
                        final_text.push_str(text);
                    }
                }
                conversation.push_message(assistant_message);
                return Ok(LoopOutcome::Done { text: final_text });
            }
            crate::response::StopReason::Refusal => {
                conversation.push_message(assistant_message);
                return Ok(LoopOutcome::Refused);
            }
            crate::response::StopReason::MaxTokens => {
                return Err(LoopError::MaxTokens);
            }
            crate::response::StopReason::ToolUse => {
                // Process tool calls
                conversation.push_message(assistant_message.clone());

                // Collect all tool results
                let mut tool_results = Vec::new();
                for block in &assistant_message.content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        // Validate input against tool schema
                        let tool_schema = conversation
                            .tools
                            .iter()
                            .find(|t| t.name == *name)
                            .map(|t| &t.input_schema);

                        let (validated_input, _is_error, error_msg) = match tool_schema {
                            Some(schema) => match validate_tool_input(input, schema) {
                                Ok(valid_input) => (valid_input, false, None),
                                Err(e) => {
                                    let msg = e.as_message();
                                    (input.clone(), true, Some(msg))
                                }
                            },
                            None => (input.clone(), false, None),
                        };

                        let (content, is_error) = if let Some(msg) = error_msg {
                            (msg, true)
                        } else {
                            // Check if the tool is read-only
                            let is_read_only = host.read_only(name);

                            if is_read_only {
                                // Read-only tools execute directly
                                match host.execute(name, &validated_input) {
                                    Ok(result) => (result, false),
                                    Err(e) => (format!("Execution error: {}", e), true),
                                }
                            } else {
                                // State-changing tools need approval
                                if host.approve(name, &validated_input) {
                                    // Approved - execute
                                    match host.execute(name, &validated_input) {
                                        Ok(result) => (result, false),
                                        Err(e) => (format!("Execution error: {}", e), true),
                                    }
                                } else {
                                    // Denied
                                    ("Tool execution denied by user".to_string(), true)
                                }
                            }
                        };

                        tool_results.push((id.clone(), content, is_error));
                    }
                }

                // Add all tool results in one user message
                let mut result_message = Message {
                    role: "user".into(),
                    content: Vec::new(),
                };
                for (tool_use_id, content, is_error) in tool_results {
                    result_message = result_message.with_content(ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    });
                }
                conversation.push_message(result_message);
                // Loop continues to next iteration
            }
            crate::response::StopReason::PauseTurn => {
                // Re-send without adding a new user message - just append the partial assistant message
                conversation.push_message(assistant_message);
                // Loop continues to next iteration
            }
        }
    }
}
