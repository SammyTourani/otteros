//! Acceptance oracle for brief M9-T0b (the agent loop that lets Claude operate OtterOS), written by
//! the orchestrator from the Messages API streaming and tool-use rules. The crate must pass this file
//! unchanged. No network: a scripted transport returns SSE bytes (split into random small chunks)
//! and records every request.
//!
//! API this file relies on (added in M9-T0b):
//!   ContentBlock::ToolResult { tool_use_id: String, content: String, is_error: bool }
//!   ContentBlock::RedactedThinking { data: String }
//!   trait Transport { fn send(&mut self, request: &otter_http::Request) -> Result<Vec<Vec<u8>>, String>; }
//!   trait ToolHost { fn read_only(&self, tool: &str) -> bool;
//!                    fn approve(&mut self, tool: &str, input: &str) -> bool;
//!                    fn execute(&mut self, tool: &str, input: &str) -> Result<String, String>; }
//!   run_tool_loop(&ClientConfig, &mut Conversation, &mut dyn Transport, &mut dyn ToolHost, max_turns)
//!     -> Result<LoopOutcome, LoopError>
//!   LoopOutcome { Done { text: String }, Refused }   LoopError { Api(String), Transport(String), MaxTokens, TooManyTurns }
//! Rules pinned here: thinking blocks (with signatures) and tool_use blocks are echoed back unchanged;
//! all tool results of a turn go in ONE user message, in tool_use order; state-changing tools need the
//! host's approval (a denial is reported to Claude as an error result, the tool never runs); tool input
//! is validated against the tool's schema before approval or execution; empty tool input means `{}`;
//! pause_turn resends the conversation with the partial assistant turn and no new user message.

use otter_claude::*;
use otter_json::Value;
use std::collections::VecDeque;

const KEY: &str = "sk-test-oracle-123";

fn js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Builds a streamed Messages API response.
struct Sse {
    out: String,
    index: usize,
}

impl Sse {
    fn new(id: &str) -> Sse {
        let mut s = Sse { out: String::new(), index: 0 };
        s.event(
            "message_start",
            &format!(
                r#"{{"type":"message_start","message":{{"id":{},"type":"message","role":"assistant","model":"claude-opus-5","content":[],"stop_reason":null,"usage":{{"input_tokens":12,"output_tokens":1}}}}}}"#,
                js(id)
            ),
        );
        s
    }
    fn event(&mut self, name: &str, data: &str) {
        self.out.push_str(&format!("event: {name}\ndata: {data}\n\n"));
    }
    fn block(mut self, start: &str, deltas: &[String]) -> Sse {
        let i = self.index;
        self.event("content_block_start", &format!(r#"{{"type":"content_block_start","index":{i},"content_block":{start}}}"#));
        self.event("ping", r#"{"type":"ping"}"#);
        for d in deltas {
            self.event("content_block_delta", &format!(r#"{{"type":"content_block_delta","index":{i},"delta":{d}}}"#));
        }
        self.event("content_block_stop", &format!(r#"{{"type":"content_block_stop","index":{i}}}"#));
        self.index += 1;
        self
    }
    fn thinking(self, text: &str, signature: &str) -> Sse {
        self.block(
            r#"{"type":"thinking","thinking":"","signature":""}"#,
            &[
                format!(r#"{{"type":"thinking_delta","thinking":{}}}"#, js(text)),
                format!(r#"{{"type":"signature_delta","signature":{}}}"#, js(signature)),
            ],
        )
    }
    fn text(self, text: &str) -> Sse {
        let (a, b) = text.split_at(text.len() / 2);
        self.block(
            r#"{"type":"text","text":""}"#,
            &[format!(r#"{{"type":"text_delta","text":{}}}"#, js(a)), format!(r#"{{"type":"text_delta","text":{}}}"#, js(b))],
        )
    }
    fn tool(self, id: &str, name: &str, input_json: &str) -> Sse {
        // Stream the input in up to three pieces; an empty input sends no input_json_delta at all.
        let n = input_json.len();
        let cuts = [0, n / 3, 2 * n / 3, n];
        let deltas: Vec<String> = cuts
            .windows(2)
            .filter(|w| w[1] > w[0])
            .map(|w| format!(r#"{{"type":"input_json_delta","partial_json":{}}}"#, js(&input_json[w[0]..w[1]])))
            .collect();
        self.block(&format!(r#"{{"type":"tool_use","id":{},"name":{},"input":{{}}}}"#, js(id), js(name)), &deltas)
    }
    fn stop(mut self, reason: &str) -> Vec<u8> {
        self.event(
            "message_delta",
            &format!(r#"{{"type":"message_delta","delta":{{"stop_reason":{},"stop_sequence":null}},"usage":{{"output_tokens":40}}}}"#, js(reason)),
        );
        self.event("message_stop", r#"{"type":"message_stop"}"#);
        self.out.into_bytes()
    }
}

struct Sent {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

struct Mock {
    responses: VecDeque<Vec<u8>>,
    sent: Vec<Sent>,
    rng: u64,
}

impl Mock {
    fn new(responses: Vec<Vec<u8>>) -> Mock {
        Mock { responses: responses.into(), sent: Vec::new(), rng: 0x9e37_79b9_7f4a_7c15 }
    }
}

impl Transport for Mock {
    fn send(&mut self, request: &otter_http::Request) -> Result<Vec<Vec<u8>>, String> {
        let body = otter_json::parse(&request.body).map_err(|e| format!("request body is not JSON: {e:?}"))?;
        self.sent.push(Sent { path: request.url.path.clone(), headers: request.headers.clone(), body });
        let bytes = self.responses.pop_front().ok_or("no more scripted responses")?;
        let mut chunks = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let n = 1 + (self.rng % 17) as usize;
            chunks.push(bytes[i..(i + n).min(bytes.len())].to_vec());
            i += n;
        }
        Ok(chunks)
    }
}

struct Host {
    log: Vec<String>,
    approvals: VecDeque<bool>,
}

impl ToolHost for Host {
    fn read_only(&self, tool: &str) -> bool {
        matches!(tool, "list_dir" | "read_file" | "screenshot")
    }
    fn approve(&mut self, tool: &str, input: &str) -> bool {
        self.log.push(format!("approve {tool} {input}"));
        self.approvals.pop_front().expect("an approval was asked that the script did not expect")
    }
    fn execute(&mut self, tool: &str, input: &str) -> Result<String, String> {
        self.log.push(format!("execute {tool} {input}"));
        Ok(format!("{tool} ok"))
    }
}

fn schema(text: &str) -> otter_json::Object {
    otter_json::parse_str(text).expect("schema").as_object().expect("object").clone()
}

fn conversation() -> Conversation {
    let path = r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#;
    Conversation::new("You operate OtterOS for the user.")
        .add_tool(Tool::new("list_dir", "List a directory", schema(path)))
        .add_tool(Tool::new("read_file", "Read a file", schema(path)))
        .add_tool(Tool::new(
            "write_file",
            "Write a file",
            schema(r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#),
        ))
        .add_tool(Tool::new("screenshot", "Capture the screen", schema(r#"{"type":"object","properties":{},"additionalProperties":false}"#)))
        .add_message(Message::user("Organise my docs"))
}

fn config() -> ClientConfig {
    ClientConfig::new(ApiKey::new(KEY))
}

/// The content blocks of a message, treating a plain string as one text block.
fn blocks(msg: &Value) -> Vec<Value> {
    match msg.get("content") {
        Some(Value::String(s)) => {
            vec![otter_json::parse_str(&format!(r#"{{"type":"text","text":{}}}"#, js(s))).unwrap()]
        }
        Some(Value::Array(a)) => a.to_vec(),
        other => panic!("message content {other:?}"),
    }
}

fn messages(s: &Sent) -> Vec<Value> {
    s.body.get("messages").and_then(Value::as_array).expect("messages").to_vec()
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or_else(|| panic!("missing {k} in {}", otter_json::to_string(v)))
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    field(v, k).as_str().unwrap_or_else(|| panic!("{k} is not a string"))
}

fn tool_result(b: &Value) -> (&str, String, bool) {
    assert_eq!(str_of(b, "type"), "tool_result");
    let content = match field(b, "content") {
        Value::String(s) => s.clone(),
        v => otter_json::to_string(v),
    };
    (str_of(b, "tool_use_id"), content, b.get("is_error").and_then(Value::as_bool).unwrap_or(false))
}

#[test]
fn a_full_agent_session() {
    let script = vec![
        Sse::new("msg_1").thinking("I'll look around.", "sig-1").text("Let me check.").tool("toolu_1", "list_dir", r#"{"path":"/docs"}"#).stop("tool_use"),
        Sse::new("msg_2").tool("toolu_2", "write_file", r#"{"path":"/docs/c.txt","content":"hi"}"#).stop("tool_use"),
        Sse::new("msg_3")
            .tool("toolu_3", "read_file", r#"{"path":"/docs/a.txt"}"#)
            .tool("toolu_4", "write_file", r#"{"path":"/docs/a.txt","content":"new"}"#)
            .stop("tool_use"),
        Sse::new("msg_4").tool("toolu_5", "write_file", r#"{"path":5,"content":"x"}"#).stop("tool_use"),
        Sse::new("msg_5").tool("toolu_6", "screenshot", "").stop("tool_use"),
        Sse::new("msg_6").text("All done.").stop("end_turn"),
    ];
    let mut mock = Mock::new(script);
    let mut host = Host { log: Vec::new(), approvals: VecDeque::from([false, true]) };
    let mut conv = conversation();
    let outcome = run_tool_loop(&config(), &mut conv, &mut mock, &mut host, 10).expect("the loop succeeds");
    assert_eq!(outcome, LoopOutcome::Done { text: "All done.".into() });
    assert_eq!(
        host.log,
        vec![
            r#"execute list_dir {"path":"/docs"}"#.to_string(),
            r#"approve write_file {"path":"/docs/c.txt","content":"hi"}"#.to_string(),
            r#"execute read_file {"path":"/docs/a.txt"}"#.to_string(),
            r#"approve write_file {"path":"/docs/a.txt","content":"new"}"#.to_string(),
            r#"execute write_file {"path":"/docs/a.txt","content":"new"}"#.to_string(),
            "execute screenshot {}".to_string(),
        ],
        "read-only tools run without approval, state-changing ones only after it; invalid input never reaches the host"
    );
    assert_eq!(mock.sent.len(), 6);

    // Every request: the right endpoint, headers and body settings.
    for s in &mock.sent {
        assert_eq!(s.path, "/v1/messages");
        let header = |name: &str| s.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        assert_eq!(header("x-api-key"), Some(KEY));
        assert_eq!(header("anthropic-version"), Some("2023-06-01"));
        let b = &s.body;
        assert_eq!(str_of(b, "model"), "claude-opus-5");
        assert_eq!(field(b, "max_tokens").as_i64(), Some(64000));
        assert_eq!(field(b, "stream").as_bool(), Some(true));
        assert_eq!(str_of(field(b, "thinking"), "type"), "adaptive");
        assert_eq!(str_of(field(b, "thinking"), "display"), "summarized");
        assert_eq!(str_of(field(b, "output_config"), "effort"), "high");
        assert_eq!(str_of(b, "system"), "You operate OtterOS for the user.");
        let tools = field(b, "tools").as_array().expect("tools");
        assert_eq!(tools.len(), 4);
        for t in tools {
            assert_eq!(field(t, "eager_input_streaming").as_bool(), Some(true), "eager input streaming on every tool");
            assert!(t.get("input_schema").and_then(Value::as_object).is_some());
        }
    }

    // Request 2: the first assistant turn is echoed unchanged, then one user message with the result.
    let m = messages(&mock.sent[1]);
    assert_eq!(m.len(), 3);
    assert_eq!(str_of(&m[0], "role"), "user");
    assert_eq!(str_of(&blocks(&m[0])[0], "text"), "Organise my docs");
    assert_eq!(str_of(&m[1], "role"), "assistant");
    let a = blocks(&m[1]);
    assert_eq!(a.len(), 3, "thinking, text and tool_use");
    assert_eq!((str_of(&a[0], "type"), str_of(&a[0], "thinking"), str_of(&a[0], "signature")), ("thinking", "I'll look around.", "sig-1"));
    assert_eq!((str_of(&a[1], "type"), str_of(&a[1], "text")), ("text", "Let me check."));
    assert_eq!((str_of(&a[2], "type"), str_of(&a[2], "id"), str_of(&a[2], "name")), ("tool_use", "toolu_1", "list_dir"));
    assert_eq!(otter_json::to_string(field(&a[2], "input")), r#"{"path":"/docs"}"#, "tool input echoed as a JSON object");
    assert_eq!(str_of(&m[2], "role"), "user");
    let r = blocks(&m[2]);
    assert_eq!(r.len(), 1);
    assert_eq!(tool_result(&r[0]), ("toolu_1", "list_dir ok".to_string(), false));

    // Request 3: the denied write is reported as an error result.
    let m = messages(&mock.sent[2]);
    assert_eq!(m.len(), 5);
    let (id, content, is_error) = tool_result(&blocks(&m[4])[0]);
    assert_eq!((id, is_error), ("toolu_2", true));
    assert!(content.to_lowercase().contains("denied"), "the denial is explained to Claude: {content:?}");

    // Request 4: two tool results, in tool_use order, in ONE user message.
    let m = messages(&mock.sent[3]);
    assert_eq!(m.len(), 7);
    assert_eq!(blocks(&m[5]).len(), 2, "the assistant turn had two tool_use blocks");
    let r = blocks(&m[6]);
    assert_eq!(r.len(), 2);
    assert_eq!(tool_result(&r[0]), ("toolu_3", "read_file ok".to_string(), false));
    assert_eq!(tool_result(&r[1]), ("toolu_4", "write_file ok".to_string(), false));

    // Request 5: input that fails the schema is an error result and never runs.
    let m = messages(&mock.sent[4]);
    let (id, content, is_error) = tool_result(&blocks(&m[8])[0]);
    assert_eq!((id, is_error), ("toolu_5", true));
    assert!(content.to_lowercase().contains("invalid"), "{content:?}");

    // Request 6: a tool with empty input is not dropped; its input is {}.
    let m = messages(&mock.sent[5]);
    let a = blocks(&m[9]);
    assert_eq!((str_of(&a[0], "type"), str_of(&a[0], "name")), ("tool_use", "screenshot"));
    assert_eq!(otter_json::to_string(field(&a[0], "input")), "{}");
    assert_eq!(tool_result(&blocks(&m[10])[0]).0, "toolu_6");

    // The conversation now holds the whole session, ending with the final assistant turn.
    assert_eq!(conv.messages.len(), 12);
}

#[test]
fn pause_turn_resends_without_a_user_message() {
    let mut mock = Mock::new(vec![Sse::new("m1").text("Working on it").stop("pause_turn"), Sse::new("m2").text("Finished.").stop("end_turn")]);
    let mut host = Host { log: Vec::new(), approvals: VecDeque::new() };
    let out = run_tool_loop(&config(), &mut conversation(), &mut mock, &mut host, 5).unwrap();
    assert_eq!(out, LoopOutcome::Done { text: "Finished.".into() });
    let m = messages(&mock.sent[1]);
    assert_eq!(m.len(), 2, "the paused assistant turn is appended, no new user message");
    assert_eq!(str_of(&m[1], "role"), "assistant");
    assert_eq!(str_of(&blocks(&m[1])[0], "text"), "Working on it");
    assert!(host.log.is_empty());
}

#[test]
fn refusal_max_tokens_api_errors_and_turn_limit() {
    let run = |script: Vec<Vec<u8>>, turns: usize| {
        let mut mock = Mock::new(script);
        let mut host = Host { log: Vec::new(), approvals: VecDeque::new() };
        run_tool_loop(&config(), &mut conversation(), &mut mock, &mut host, turns)
    };
    assert_eq!(run(vec![Sse::new("m").text("I can't help with that.").stop("refusal")], 5), Ok(LoopOutcome::Refused));
    assert_eq!(run(vec![Sse::new("m").text("Truncat").stop("max_tokens")], 5), Err(LoopError::MaxTokens));
    let mut err = Sse::new("m");
    err.event("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
    match run(vec![err.out.into_bytes()], 5) {
        Err(LoopError::Api(msg)) => assert!(msg.contains("Overloaded") || msg.contains("overloaded"), "{msg}"),
        other => panic!("an API error event must end the loop: {other:?}"),
    }
    let looping: Vec<Vec<u8>> = (0..3).map(|i| Sse::new(&format!("m{i}")).tool(&format!("t{i}"), "list_dir", r#"{"path":"/"}"#).stop("tool_use")).collect();
    assert_eq!(run(looping, 3), Err(LoopError::TooManyTurns));
    assert!(matches!(run(vec![], 3), Err(LoopError::Transport(_))), "a transport failure is reported, not a panic");
}

#[test]
fn the_api_key_never_leaks() {
    let c = config();
    assert!(!format!("{c:?}").contains(KEY), "Debug of the config redacts the key");
    let mut mock = Mock::new(vec![]);
    let mut host = Host { log: Vec::new(), approvals: VecDeque::new() };
    let e = run_tool_loop(&c, &mut conversation(), &mut mock, &mut host, 3).unwrap_err();
    assert!(!format!("{e:?}").contains(KEY));
}
