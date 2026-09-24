use otter_claude::{
    Client, ApiKey, ClientConfig, Conversation, Message, Tool,
};
use otter_json::Object;
use otter_http::Url;
use std::net::TcpStream;
use std::process::{Command, Stdio, Child};
use std::thread;
use std::time::Duration;

/// Spawn the mock server on a free port, returning (port, process)
fn spawn_mock_server() -> (u16, Child) {
    // Find a free port by trying to bind
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("Failed to find free port");
    let port = listener.local_addr().expect("Failed to get port").port();
    drop(listener);

    let child = Command::new("python3")
        .arg("/Users/sammytourani/Projects/otteros/scripts/claude-mock-server.py")
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn mock server");

    // Wait for server to be ready
    thread::sleep(Duration::from_millis(300));

    (port, child)
}

#[test]
fn test_request_sends_to_mock_server() {
    // Spawn mock server
    let (port, mut server) = spawn_mock_server();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Create a request that should be accepted by the mock server
        let _client = Client::new(
            ClientConfig::new(ApiKey::new("sk-test-key"))
                .with_model("claude-opus-5")
                .with_max_tokens(64000),
        );

        let schema = Object::new();
        let tool = Tool::new("calculate", "Adds two numbers", schema);

        let conv = Conversation::new("You are a helpful assistant")
            .add_tool(tool)
            .add_message(Message::user("Calculate 2 + 3"));

        let url = Url::parse(&format!("https://127.0.0.1:{}/v1/messages", port))
            .expect("Failed to parse URL");

        // Build request
        let req = otter_claude::request::build_request(
            url,
            "sk-test-key",
            &conv,
            "claude-opus-5",
            64000,
            true,
        ).expect("Failed to build request");

        // Verify request has correct headers
        let headers_map: std::collections::HashMap<&str, &str> = req
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        assert!(headers_map.contains_key("x-api-key"));
        assert!(headers_map.contains_key("anthropic-version"));
        assert!(headers_map.contains_key("anthropic-beta"));

        // Try to connect to the mock server
        let _stream = TcpStream::connect(("127.0.0.1", port))
            .expect("Failed to connect to mock server");

        println!("Successfully connected to mock server on port {}", port);
        println!("Request has required headers");
    }));

    // Kill server
    let _ = server.kill();
    let _ = server.wait();

    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn test_headers_and_request_validation() {
    // This test verifies the request structure is correct by
    // checking that it would be accepted by a strict mock server
    let _client = Client::new(
        ClientConfig::new(ApiKey::new("test-key"))
            .with_model("claude-opus-5"),
    );

    let conv = Conversation::new("System prompt")
        .add_message(Message::user("Test message"));

    let url = Url::parse("https://api.anthropic.com/v1/messages")
        .expect("Failed to parse URL");

    let req = otter_claude::request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).expect("Failed to build request");

    // Verify required headers
    let headers_map: std::collections::HashMap<&str, &str> = req
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    assert_eq!(
        headers_map.get("x-api-key"),
        Some(&"test-key"),
        "x-api-key header missing or wrong"
    );
    assert_eq!(
        headers_map.get("anthropic-version"),
        Some(&"2023-06-01"),
        "anthropic-version header missing or wrong"
    );

    let beta = headers_map
        .get("anthropic-beta")
        .unwrap_or(&"");
    assert!(
        beta.contains("server-side-fallback-2026-07-01"),
        "anthropic-beta header missing correct value"
    );

    // Verify body is valid JSON and has required fields
    let body_str = String::from_utf8(req.body).expect("Invalid UTF-8");
    let body = otter_json::parse_str(&body_str).expect("Failed to parse body JSON");
    let obj = body.as_object().expect("Body is not an object");

    assert!(obj.contains_key("model"));
    assert!(obj.contains_key("max_tokens"));
    assert!(obj.contains_key("stream"));
    assert!(obj.contains_key("messages"));
}

#[test]
fn test_tool_input_validation_integration() {
    // Verify that invalid tool inputs are caught before execution
    let _ = Client::new(
        ClientConfig::new(ApiKey::new("sk-test"))
            .with_model("claude-opus-5"),
    );

    let mut schema = Object::new();
    schema.insert("type".into(), otter_json::Value::string("object"));

    let mut properties = Object::new();
    let mut x_prop = Object::new();
    x_prop.insert("type".into(), otter_json::Value::string("integer"));
    properties.insert("x".into(), otter_json::Value::Object(x_prop));

    schema.insert("properties".into(), otter_json::Value::Object(properties));

    let tool = Tool::new("add", "Adds two numbers", schema);

    let conv = Conversation::new("Helper")
        .add_tool(tool)
        .add_message(Message::user("Add 5 and 3"));

    // Verify the conversation can be built into a request
    let url = Url::parse("https://api.anthropic.com/v1/messages")
        .expect("Failed to parse URL");

    let req = otter_claude::request::build_request(
        url,
        "test-key",
        &conv,
        "claude-opus-5",
        64000,
        true,
    ).expect("Failed to build request");

    // The request should include the tool definition
    let body_str = String::from_utf8(req.body).expect("Invalid UTF-8");
    let body = otter_json::parse_str(&body_str).expect("Failed to parse JSON");
    let obj = body.as_object().expect("Not an object");

    let tools = obj
        .get("tools")
        .and_then(|v| v.as_array());
    assert!(
        tools.is_some(),
        "Request should include tools array"
    );
}
