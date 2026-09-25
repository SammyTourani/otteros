use alloc::string::{String, ToString};
use alloc::vec::Vec;
use otter_json::parse_str;
use otter_http::SseParser;

/// Events produced during streaming a turn from the API.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Text content delta
    Text {
        /// The text chunk
        delta: String,
    },
    /// Thinking content delta
    Thinking {
        /// The thinking content
        delta: String,
    },
    /// Thinking block with signature (final, complete block)
    ThinkingWithSignature {
        /// The complete thinking content
        content: String,
        /// The signature
        signature: String,
    },
    /// Tool use is starting
    ToolUseStart {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
    },
    /// Tool use input is ready
    ToolUseReady {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
        /// Parsed tool input
        input: String,
    },
    /// Turn has stopped
    Stop {
        /// Stop reason: end_turn, tool_use, max_tokens, refusal, pause_turn
        reason: StopReason,
    },
    /// An error occurred
    Error {
        /// Error message
        message: String,
    },
}

/// Reasons a turn might stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Model finished naturally
    EndTurn,
    /// Model requested tool calls
    ToolUse,
    /// Model hit max tokens
    MaxTokens,
    /// Model refused the request
    Refusal,
    /// Model paused for turn continuation
    PauseTurn,
}

impl StopReason {
    /// Parse a stop reason string from the API
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "end_turn" => Some(StopReason::EndTurn),
            "tool_use" => Some(StopReason::ToolUse),
            "max_tokens" => Some(StopReason::MaxTokens),
            "refusal" => Some(StopReason::Refusal),
            "pause_turn" => Some(StopReason::PauseTurn),
            _ => None,
        }
    }
}

/// Result of a turn in the conversation.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// Message ID from the API
    pub message_id: String,
    /// Model that served the response
    pub model: String,
    /// Stop reason
    pub stop_reason: StopReason,
    /// Input tokens used
    pub input_tokens: u32,
    /// Output tokens used
    pub output_tokens: u32,
    /// All events from this turn (for reconstruction)
    pub events: Vec<TurnEvent>,
}

/// Parses Claude API SSE stream events.
#[allow(dead_code)]
pub struct StreamParser {
    sse: SseParser,
    current_message_id: String,
    current_model: String,
    input_tokens: u32,
    output_tokens: u32,
    stop_reason: Option<StopReason>,
    events: Vec<TurnEvent>,
    // Track current tool use being built
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
    current_tool_has_input_delta: bool,
    // Track current thinking block
    current_thinking: String,
    current_signature: String,
    in_thinking_block: bool,
}

impl StreamParser {
    /// Creates a new stream parser.
    pub fn new() -> Self {
        StreamParser {
            sse: SseParser::new(),
            current_message_id: String::new(),
            current_model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: None,
            events: Vec::new(),
            current_tool_id: None,
            current_tool_name: None,
            current_tool_input: String::new(),
            current_tool_has_input_delta: false,
            current_thinking: String::new(),
            current_signature: String::new(),
            in_thinking_block: false,
        }
    }

    /// Feed data to the parser.
    pub fn feed(&mut self, data: &[u8]) {
        self.sse.feed(data);
    }

    /// Try to get the next parsed event.
    #[allow(clippy::question_mark, clippy::collapsible_if)]
    pub fn next_event(&mut self) -> Option<TurnEvent> {
        while let Some(sse_event) = self.sse.next_event() {
                if sse_event.event == "message_start" {
                    // Parse message_start event
                    if let Ok(data) = parse_str(&sse_event.data)
                        && let Some(obj) = data.as_object()
                    {
                        // Get the message object
                        if let Some(message) = obj.get("message").and_then(|v| v.as_object()) {
                            if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
                                self.current_message_id = id.to_string();
                            }
                            if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                                self.current_model = model.to_string();
                            }
                        }
                    }
                } else if sse_event.event == "content_block_start" {
                    // Parse content_block_start event
                    if let Ok(data) = parse_str(&sse_event.data)
                        && let Some(obj) = data.as_object()
                        && let Some(block) = obj.get("content_block")
                        && let Some(block_obj) = block.as_object()
                        && let Some(block_type) = block_obj.get("type").and_then(|v| v.as_str())
                    {
                        match block_type {
                            "tool_use" => {
                                if let Some(id) = block_obj.get("id").and_then(|v| v.as_str()) {
                                    self.current_tool_id = Some(id.to_string());
                                }
                                if let Some(name) = block_obj.get("name").and_then(|v| v.as_str()) {
                                    self.current_tool_name = Some(name.to_string());
                                    self.current_tool_input.clear();
                                    self.current_tool_has_input_delta = false;
                                    let event = TurnEvent::ToolUseStart {
                                        id: self.current_tool_id.clone().unwrap_or_default(),
                                        name: name.to_string(),
                                    };
                                    self.events.push(event.clone());
                                    return Some(event);
                                }
                            }
                            "thinking" => {
                                self.current_thinking.clear();
                                self.current_signature.clear();
                                self.in_thinking_block = true;
                            }
                            _ => {}
                        }
                    }
                } else if sse_event.event == "content_block_delta" {
                    // Parse content_block_delta event
                    if let Ok(data) = parse_str(&sse_event.data)
                        && let Some(obj) = data.as_object()
                        && let Some(delta) = obj.get("delta")
                        && let Some(delta_obj) = delta.as_object()
                        && let Some(delta_type) = delta_obj.get("type").and_then(|v| v.as_str())
                    {
                        match delta_type {
                            "text_delta" => {
                                if let Some(text) = delta_obj.get("text").and_then(|v| v.as_str()) {
                                    let event = TurnEvent::Text { delta: text.to_string() };
                                    self.events.push(event.clone());
                                    return Some(event);
                                }
                            }
                            "thinking_delta" => {
                                if let Some(text) = delta_obj.get("thinking").and_then(|v| v.as_str()) {
                                    self.current_thinking.push_str(text);
                                    let event = TurnEvent::Thinking { delta: text.to_string() };
                                    self.events.push(event.clone());
                                    return Some(event);
                                }
                            }
                            "input_json_delta" => {
                                if let Some(partial) = delta_obj.get("partial_json").and_then(|v| v.as_str()) {
                                    self.current_tool_input.push_str(partial);
                                    self.current_tool_has_input_delta = true;
                                }
                            }
                            "signature_delta" => {
                                // Signature deltas are part of thinking blocks
                                if let Some(sig) = delta_obj.get("signature").and_then(|v| v.as_str()) {
                                    self.current_signature.push_str(sig);
                                }
                            }
                            _ => {}
                        }
                    }
                } else if sse_event.event == "content_block_stop" {
                    // Tool use block complete
                    if let (Some(id), Some(name)) = (self.current_tool_id.take(), self.current_tool_name.take()) {
                        // Emit the tool use ready event even if input is empty (empty input becomes {})
                        let input = if self.current_tool_has_input_delta || !self.current_tool_input.is_empty() {
                            self.current_tool_input.clone()
                        } else {
                            "{}".to_string()
                        };
                        let event = TurnEvent::ToolUseReady {
                            id,
                            name,
                            input,
                        };
                        self.events.push(event.clone());
                        self.current_tool_input.clear();
                        self.current_tool_has_input_delta = false;
                        return Some(event);
                    }
                    // Thinking block complete - emit final thinking with signature (if we have any thinking content)
                    if self.in_thinking_block && !self.current_thinking.is_empty() {
                        self.in_thinking_block = false;
                        // Emit a final Thinking event with signature (signature can be empty)
                        let event = TurnEvent::ThinkingWithSignature {
                            content: self.current_thinking.clone(),
                            signature: self.current_signature.clone(),
                        };
                        self.events.push(event.clone());
                        self.current_thinking.clear();
                        self.current_signature.clear();
                        return Some(event);
                    } else if self.in_thinking_block {
                        self.in_thinking_block = false;
                        self.current_thinking.clear();
                        self.current_signature.clear();
                    }
                } else if sse_event.event == "message_delta" {
                    // Parse message_delta for usage and stop_reason
                    if let Ok(data) = parse_str(&sse_event.data)
                        && let Some(obj) = data.as_object()
                    {
                        if let Some(usage) = obj.get("usage")
                            && let Some(usage_obj) = usage.as_object()
                            && let Some(output) = usage_obj.get("output_tokens").and_then(|v| v.as_i64())
                        {
                            self.output_tokens = output as u32;
                        }
                        // stop_reason is inside the delta object
                        if let Some(delta) = obj.get("delta")
                            && let Some(delta_obj) = delta.as_object()
                            && let Some(stop_reason_str) = delta_obj.get("stop_reason").and_then(|v| v.as_str())
                            && let Some(reason) = StopReason::from_str(stop_reason_str)
                        {
                            self.stop_reason = Some(reason);
                        }
                    }
                } else if sse_event.event == "message_stop" {
                    // End of stream
                    let reason = self.stop_reason.unwrap_or(StopReason::EndTurn);
                    let event = TurnEvent::Stop { reason };
                    self.events.push(event.clone());
                    return Some(event);
                } else if sse_event.event == "error" {
                    // Parse error event
                    if let Ok(data) = parse_str(&sse_event.data)
                        && let Some(error_obj) = data.as_object()
                        && let Some(error_info) = error_obj.get("error")
                        && let Some(error_details) = error_info.as_object()
                        && let Some(message) = error_details.get("message").and_then(|v| v.as_str())
                    {
                        let event = TurnEvent::Error {
                            message: message.to_string(),
                        };
                        self.events.push(event.clone());
                        return Some(event);
                    }
                } else if sse_event.event == "ping" {
                    // Ignore ping events
                    continue;
                } else {
                    // Unknown event type - preserve but ignore
                    continue;
                }
        }
        None
    }

    /// Finalize and return the turn result.
    pub fn finalize(self) -> TurnResult {
        TurnResult {
            message_id: self.current_message_id,
            model: self.current_model,
            stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            events: self.events,
        }
    }
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}
