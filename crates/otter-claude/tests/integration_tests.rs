use otter_claude::{
    Client, ApiKey, ClientConfig, Conversation, Message, ContentBlock, Tool, TurnEvent, StopReason,
};
use otter_json::{Object, Value};

#[test]
fn test_api_key_redaction() {
    let key = ApiKey::new("sk-test-secret-12345");
    assert_eq!(format!("{:?}", key), "<redacted>");
    assert_eq!(format!("{}", key), "<redacted>");
}

#[test]
fn test_client_creation() {
    let config = ClientConfig::new(ApiKey::new("test-key"));
    let client = Client::new(config);
    // Just verify it creates without panic
    let _ = client;
}

#[test]
fn test_conversation_building() {
    let conv = Conversation::new("You are a helpful assistant")
        .add_tool(Tool::new(
            "echo",
            "Echoes the input",
            Object::new(),
        ))
        .add_message(Message::user("Hello!"));

    assert_eq!(conv.system, "You are a helpful assistant");
    assert_eq!(conv.tools.len(), 1);
    assert_eq!(conv.messages.len(), 1);
}

#[test]
fn test_content_blocks() {
    let mut msg = Message::assistant()
        .with_content(ContentBlock::Text("Hello".into()))
        .with_content(ContentBlock::Thinking {
            content: "I'm thinking".into(),
            signature: None,
        })
        .with_content(ContentBlock::ToolUse {
            id: "tool_123".into(),
            name: "calculator".into(),
            input: r#"{"a": 1, "b": 2}"#.into(),
        });

    assert_eq!(msg.content.len(), 3);
    assert_eq!(msg.role, "assistant");

    // Add more content
    msg = msg.with_content(ContentBlock::Fallback {
        reason: "Model switched".into(),
    });
    assert_eq!(msg.content.len(), 4);
}

#[test]
fn test_sse_parsing() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut event_count = 0;
    let mut found_text = false;
    let mut found_stop = false;

    while let Some(event) = parser.next_event() {
        match event {
            TurnEvent::Text { delta } => {
                found_text = true;
                assert_eq!(delta, "Hello");
            }
            TurnEvent::Stop { reason } => {
                found_stop = true;
                assert_eq!(reason, StopReason::EndTurn);
            }
            _ => {}
        }
        event_count += 1;
    }

    assert!(found_text, "Should find text event");
    assert!(found_stop, "Should find stop event");
    assert!(event_count > 0, "Should parse at least one event");
}

#[test]
fn test_tool_use_parsing() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_123", "name": "calculator"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "\"x\": 1"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut found_start = false;
    let mut found_ready = false;

    while let Some(event) = parser.next_event() {
        match event {
            TurnEvent::ToolUseStart { id, name } => {
                found_start = true;
                assert_eq!(id, "tool_123");
                assert_eq!(name, "calculator");
            }
            TurnEvent::ToolUseReady { id, name, input } => {
                found_ready = true;
                assert_eq!(id, "tool_123");
                assert_eq!(name, "calculator");
                assert!(input.contains("x"));
            }
            _ => {}
        }
    }

    assert!(found_start, "Should find tool use start");
    assert!(found_ready, "Should find tool use ready");
}

#[test]
fn test_error_parsing() {
    let sse_data = r#"event: error
data: {"error": {"type": "invalid_request_error", "message": "Invalid API key"}}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut found_error = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Error { message } = event {
            found_error = true;
            assert_eq!(message, "Invalid API key");
        }
    }

    assert!(found_error, "Should find error event");
}

#[test]
fn test_unknown_events_ignored() {
    let sse_data = r#"event: custom_event
data: {"some": "data"}

event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: unknown_block_type
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "unknown"}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let events: Vec<_> = core::iter::from_fn(|| parser.next_event()).collect();
    // Should handle unknown events gracefully without panicking
    let _ = events;
}

#[test]
fn test_tool_input_validation_success() {
    let input = r#"{"x": 1, "y": 2}"#;

    let mut schema = Object::new();
    schema.insert("type".into(), Value::string("object"));

    let mut properties = Object::new();
    let mut x_prop = Object::new();
    x_prop.insert("type".into(), Value::string("integer"));
    properties.insert("x".into(), Value::Object(x_prop));

    let mut y_prop = Object::new();
    y_prop.insert("type".into(), Value::string("integer"));
    properties.insert("y".into(), Value::Object(y_prop));

    schema.insert("properties".into(), Value::Object(properties));

    let result = otter_claude::validation::validate_tool_input(input, &schema);
    assert!(result.is_ok(), "Valid input should pass");
}

#[test]
fn test_tool_input_validation_invalid_json() {
    let input = r#"{"x": 1, "y": invalid}"#;

    let schema = Object::new();
    let result = otter_claude::validation::validate_tool_input(input, &schema);
    assert!(result.is_err(), "Invalid JSON should fail");

    if let Err(e) = result {
        assert_eq!(
            e.as_message(),
            "INVALID_JSON: Failed to parse input"
        );
    }
}

#[test]
fn test_tool_input_validation_missing_required() {
    let input = r#"{"y": 2}"#;

    let mut schema = Object::new();
    schema.insert("type".into(), Value::string("object"));

    let mut properties = Object::new();
    let mut x_prop = Object::new();
    x_prop.insert("type".into(), Value::string("integer"));
    properties.insert("x".into(), Value::Object(x_prop));
    schema.insert("properties".into(), Value::Object(properties));

    let required_array = vec![Value::string("x")];
    schema.insert("required".into(), Value::Array(required_array));

    let result = otter_claude::validation::validate_tool_input(input, &schema);
    assert!(result.is_err(), "Missing required property should fail");
}

#[test]
fn test_tool_input_validation_type_error() {
    let input = r#"{"x": "not a number"}"#;

    let mut schema = Object::new();
    schema.insert("type".into(), Value::string("object"));

    let mut properties = Object::new();
    let mut x_prop = Object::new();
    x_prop.insert("type".into(), Value::string("integer"));
    properties.insert("x".into(), Value::Object(x_prop));
    schema.insert("properties".into(), Value::Object(properties));

    let result = otter_claude::validation::validate_tool_input(input, &schema);
    assert!(result.is_err(), "Type mismatch should fail");
}

#[test]
fn test_tool_input_validation_extra_properties() {
    let input = r#"{"x": 1, "z": "extra"}"#;

    let mut schema = Object::new();
    schema.insert("type".into(), Value::string("object"));
    schema.insert("additionalProperties".into(), Value::Bool(false));

    let mut properties = Object::new();
    let mut x_prop = Object::new();
    x_prop.insert("type".into(), Value::string("integer"));
    properties.insert("x".into(), Value::Object(x_prop));
    schema.insert("properties".into(), Value::Object(properties));

    let result = otter_claude::validation::validate_tool_input(input, &schema);
    assert!(result.is_err(), "Extra property should fail when additionalProperties is false");
}

#[test]
fn test_fragmented_json_handling() {
    // Test that streaming JSON deltas can be reassembled
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_123", "name": "parse_json"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"key\""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": ": \"val"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "ue\"}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut complete_input = String::new();
    while let Some(event) = parser.next_event() {
        if let TurnEvent::ToolUseReady { input, .. } = event {
            complete_input = input;
        }
    }

    // Verify the fragmented JSON was reassembled correctly
    assert!(complete_input.contains("key"));
    assert!(complete_input.contains("value"));
}

#[test]
fn test_parallel_tool_calls() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_1", "name": "calc"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"x\": 1}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "tool_2", "name": "calc"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"x\": 2}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 20}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut tool_count = 0;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::ToolUseReady { .. } = event {
            tool_count += 1;
        }
    }

    assert_eq!(tool_count, 2, "Should handle two parallel tool calls");
}

#[test]
fn test_refusal_stop_reason() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "I can't help with that."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "refusal"}, "usage": {"output_tokens": 15}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut found_refusal = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Stop { reason: StopReason::Refusal } = event {
            found_refusal = true;
        }
    }

    assert!(found_refusal, "Should detect refusal stop reason");
}

#[test]
fn test_thinking_with_signature() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_123", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Let me analyze this..."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 20}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut found_thinking = false;
    while let Some(event) = parser.next_event() {
        if let TurnEvent::Thinking { .. } = event {
            found_thinking = true;
        }
    }

    assert!(found_thinking, "Should handle thinking blocks");
}
