use otter_claude::{request, Conversation, Message, ContentBlock, Tool};
use otter_json::{Object, Value, parse_str};
use otter_http::Url;

#[test]
fn test_request_building_basic() {
    let conv = Conversation::new("You are a helpful assistant")
        .add_message(Message::user("Hello!"));

    let url = Url::parse("https://api.anthropic.com/v1/messages").unwrap();
    let req = request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).unwrap();

    // Check request method and URL
    assert_eq!(req.method, otter_http::Method::Post);

    // Check headers
    let header_map: std::collections::HashMap<&str, &str> = req.headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    assert_eq!(header_map.get("content-type"), Some(&"application/json"));
    assert_eq!(header_map.get("x-api-key"), Some(&"test-key"));
    assert_eq!(header_map.get("anthropic-version"), Some(&"2023-06-01"));
    assert!(header_map.get("anthropic-beta").unwrap_or(&"").contains("server-side-fallback-2026-07-01"));

    // Check body is valid JSON
    let body_str = String::from_utf8(req.body).unwrap();
    let body_json = parse_str(&body_str).unwrap();
    let body_obj = body_json.as_object().unwrap();

    // Check required fields
    assert_eq!(body_obj.get("model").unwrap().as_str().unwrap(), "claude-opus-5");
    assert_eq!(body_obj.get("max_tokens").unwrap().as_i64().unwrap(), 64000);
    assert!(body_obj.get("stream").unwrap().as_bool().unwrap());
    assert_eq!(body_obj.get("system").unwrap().as_str().unwrap(), "You are a helpful assistant");
    assert_eq!(body_obj.get("fallbacks").unwrap().as_str().unwrap(), "default");

    // Check thinking config
    let thinking = body_obj.get("thinking").unwrap().as_object().unwrap();
    assert_eq!(thinking.get("type").unwrap().as_str().unwrap(), "adaptive");
    assert_eq!(thinking.get("display").unwrap().as_str().unwrap(), "summarized");

    // Check output config
    let output_config = body_obj.get("output_config").unwrap().as_object().unwrap();
    assert_eq!(output_config.get("effort").unwrap().as_str().unwrap(), "high");

    // Check messages
    let messages = body_obj.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 1);

    let msg = messages[0].as_object().unwrap();
    assert_eq!(msg.get("role").unwrap().as_str().unwrap(), "user");

    let content = msg.get("content").unwrap().as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0].as_object().unwrap().get("type").unwrap().as_str().unwrap(), "text");
    assert_eq!(content[0].as_object().unwrap().get("text").unwrap().as_str().unwrap(), "Hello!");
}

#[test]
fn test_request_building_with_tools() {
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

    let tool = Tool::new("add", "Add two numbers", schema);

    let conv = Conversation::new("You are a math helper")
        .add_tool(tool)
        .add_message(Message::user("What is 2 + 3?"));

    let url = Url::parse("https://api.anthropic.com/v1/messages").unwrap();
    let req = request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).unwrap();

    let body_str = String::from_utf8(req.body).unwrap();
    let body_json = parse_str(&body_str).unwrap();
    let body_obj = body_json.as_object().unwrap();

    // Check tools
    let tools = body_obj.get("tools").unwrap().as_array().unwrap();
    assert_eq!(tools.len(), 1);

    let tool_obj = tools[0].as_object().unwrap();
    assert_eq!(tool_obj.get("name").unwrap().as_str().unwrap(), "add");
    assert_eq!(tool_obj.get("description").unwrap().as_str().unwrap(), "Add two numbers");
    assert!(tool_obj.get("eager_input_streaming").unwrap().as_bool().unwrap());

    let input_schema = tool_obj.get("input_schema").unwrap().as_object().unwrap();
    assert_eq!(input_schema.get("type").unwrap().as_str().unwrap(), "object");
}

#[test]
fn test_request_building_with_thinking_blocks() {
    let msg = Message::assistant()
        .with_content(ContentBlock::Thinking {
            content: "Let me think about this...".into(),
            signature: Some("sig_123".into()),
        })
        .with_content(ContentBlock::Text("The answer is 42.".into()));

    let conv = Conversation::new("Assistant")
        .add_message(msg);

    let url = Url::parse("https://api.anthropic.com/v1/messages").unwrap();
    let req = request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).unwrap();

    let body_str = String::from_utf8(req.body).unwrap();
    let body_json = parse_str(&body_str).unwrap();
    let body_obj = body_json.as_object().unwrap();

    let messages = body_obj.get("messages").unwrap().as_array().unwrap();
    let msg_obj = messages[0].as_object().unwrap();
    let content = msg_obj.get("content").unwrap().as_array().unwrap();

    // Check thinking block
    let thinking_block = content[0].as_object().unwrap();
    assert_eq!(thinking_block.get("type").unwrap().as_str().unwrap(), "thinking");
    assert_eq!(thinking_block.get("thinking").unwrap().as_str().unwrap(), "Let me think about this...");
    assert_eq!(thinking_block.get("signature").unwrap().as_str().unwrap(), "sig_123");

    // Check text block
    let text_block = content[1].as_object().unwrap();
    assert_eq!(text_block.get("type").unwrap().as_str().unwrap(), "text");
    assert_eq!(text_block.get("text").unwrap().as_str().unwrap(), "The answer is 42.");
}

#[test]
fn test_request_building_two_turn_conversation() {
    // First turn: user message
    // Second turn: assistant message with tool use
    // Third turn: user message with tool result

    let conv = Conversation::new("Helper")
        .add_message(Message::user("Calculate 2 + 3"))
        .add_message(
            Message::assistant()
                .with_content(ContentBlock::ToolUse {
                    id: "tool_123".into(),
                    name: "add".into(),
                    input: r#"{"x": 2, "y": 3}"#.into(),
                })
        )
        .add_message(Message::user("Tool result is 5"));

    let url = Url::parse("https://api.anthropic.com/v1/messages").unwrap();
    let req = request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).unwrap();

    let body_str = String::from_utf8(req.body).unwrap();
    let body_json = parse_str(&body_str).unwrap();
    let body_obj = body_json.as_object().unwrap();

    let messages = body_obj.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 3);

    // Check first message is user
    assert_eq!(messages[0].as_object().unwrap().get("role").unwrap().as_str().unwrap(), "user");

    // Check second message is assistant with tool use
    assert_eq!(messages[1].as_object().unwrap().get("role").unwrap().as_str().unwrap(), "assistant");
    let content = messages[1].as_object().unwrap().get("content").unwrap().as_array().unwrap();
    let tool_block = content[0].as_object().unwrap();
    assert_eq!(tool_block.get("type").unwrap().as_str().unwrap(), "tool_use");
    assert_eq!(tool_block.get("id").unwrap().as_str().unwrap(), "tool_123");
    assert_eq!(tool_block.get("name").unwrap().as_str().unwrap(), "add");

    // Check third message is user
    assert_eq!(messages[2].as_object().unwrap().get("role").unwrap().as_str().unwrap(), "user");
}

#[test]
fn test_request_fallbacks_disabled() {
    let conv = Conversation::new("Helper")
        .add_message(Message::user("Hello"));

    let url = Url::parse("https://api.anthropic.com/v1/messages").unwrap();
    let req = request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        false, // fallbacks disabled
    ).unwrap();

    let body_str = String::from_utf8(req.body).unwrap();
    let body_json = parse_str(&body_str).unwrap();
    let body_obj = body_json.as_object().unwrap();

    // Check that fallbacks field is not present
    assert!(!body_obj.contains_key("fallbacks"));
}
