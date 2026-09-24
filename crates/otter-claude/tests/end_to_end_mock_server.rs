use otter_claude::{
    Client, ApiKey, ClientConfig, Conversation, Message, ContentBlock, Tool, TurnEvent,
};
use otter_json::Object;
use otter_http::Url;

#[test]
fn test_request_with_api_key_and_headers() {
    let url = Url::parse("https://api.anthropic.com/v1/messages")
        .expect("Failed to parse URL");

    let config = ClientConfig::new(ApiKey::new("test-key-12345"))
        .with_model("claude-opus-5")
        .with_max_tokens(64000);

    let _client = Client::new(config);

    // Build a conversation with a tool
    let mut schema = Object::new();
    schema.insert("type".into(), otter_json::Value::string("object"));

    let tool = Tool::new("calculator", "A simple calculator", schema);

    let conv = Conversation::new("You are a helpful assistant")
        .add_tool(tool)
        .add_message(Message::user("Calculate 2 + 2"));

    // Build request
    let req = otter_claude::request::build_request(
        url,
        "test-key-12345",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).expect("Failed to build request");

    // Verify request structure
    assert_eq!(req.method, otter_http::Method::Post);

    // Check headers are present
    let mut found_auth = false;
    let mut found_version = false;
    for (name, value) in &req.headers {
        if name == "x-api-key" && value == "test-key-12345" {
            found_auth = true;
        }
        if name == "anthropic-version" && value == "2023-06-01" {
            found_version = true;
        }
    }
    assert!(found_auth, "x-api-key header missing");
    assert!(found_version, "anthropic-version header missing");

    // Verify body JSON is valid
    let body_str = String::from_utf8(req.body).expect("Invalid UTF-8 in body");
    let _: otter_json::Value = otter_json::parse_str(&body_str)
        .expect("Failed to parse request body JSON");
}

#[test]
fn test_headers_validation() {
    let config = ClientConfig::new(ApiKey::new("sk-test-secret-key"))
        .with_model("claude-opus-5");

    let config_debug = format!("{:?}", config);

    // Verify the secret key never appears in the debug output
    assert!(
        !config_debug.contains("sk-test-secret-key"),
        "Secret key appeared in ClientConfig debug output!"
    );
    assert!(
        config_debug.contains("<redacted>"),
        "ClientConfig should show <redacted> for the API key"
    );

    // Also verify in the API key directly
    let key = ApiKey::new("sk-another-secret");
    let key_debug = format!("{:?}", key);
    assert_eq!(key_debug, "<redacted>");

    let key_display = format!("{}", key);
    assert_eq!(key_display, "<redacted>");
}

#[test]
fn test_streaming_with_thinking() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_001", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "I need to think about this"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "The answer is clear."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 20}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    let mut thinking_found = false;
    let mut text_found = false;
    let mut stop_found = false;

    while let Some(event) = parser.next_event() {
        match event {
            TurnEvent::Thinking { .. } => thinking_found = true,
            TurnEvent::Text { .. } => text_found = true,
            TurnEvent::Stop { .. } => stop_found = true,
            _ => {}
        }
    }

    assert!(thinking_found, "Should find thinking blocks");
    assert!(text_found, "Should find text blocks");
    assert!(stop_found, "Should find stop event");
}

#[test]
fn test_tool_result_structure() {
    // Verify that a user message with tool_result can be built
    let msg = Message::user("Tool result: 42")
        .with_content(ContentBlock::Text("Result received".into()));

    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.len(), 2); // Original text + added text

    // In the real API, tool_result would be a specific content type,
    // but for now we test that the message structure is sound
}

#[test]
fn test_fallback_block_handling() {
    let sse_data = r#"event: message_start
data: {"message": {"id": "msg_001", "model": "claude-3-sonnet"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "fallback", "reason": "overloaded"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Fallback response"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"#;

    let mut parser = otter_claude::response::StreamParser::new();
    parser.feed(sse_data.as_bytes());

    // Parse all events to populate the fields
    let _events: Vec<_> = core::iter::from_fn(|| parser.next_event()).collect();

    let result = parser.finalize();

    // The model field should reflect the fallback model
    assert_eq!(result.model, "claude-3-sonnet", "Should have model name from message_start");
}
