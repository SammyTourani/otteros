//! Acceptance oracle for brief M9-T0e (otter-agent: Claude operates OtterOS through its tools,
//! over OtterOS's own TLS 1.3 and HTTP/1.1), written by the orchestrator. The crate must pass this
//! file unchanged.
//!
//! Every test talks to scripts/mock_claude.py (D23: never the real API) over a real TCP socket and
//! a real TLS 1.3 handshake with the test PKI (leaf.otter-test.example, root from
//! crates/otter-x509/tests/fixtures/pki/root.der). The mock validates every request (headers,
//! model, tools, echoed thinking signatures, exact tool results) and logs it.
//!
//! API this file relies on (crate `otter_agent`):
//!   trait ByteStream { fn read(&mut self, buf: &mut [u8]) -> Result<usize, String>;   // Ok(0) = closed
//!                      fn write_all(&mut self, data: &[u8]) -> Result<(), String>; }
//!   trait Connector { fn connect(&mut self, host: &str, port: u16) -> Result<Box<dyn ByteStream>, String>; }
//!   Backend { host: String, port: u16, server_name: String, path: String, api_key: String,
//!             roots_der: Vec<Vec<u8>>, now_unix: u64 }
//!   Agent::new(Backend, Box<dyn Connector>, Box<dyn otter_tls::Rng>, otter_tools::Session<O>, system: &str)
//!   Agent::ask(&mut self, user_text: &str) -> Result<String, AgentError>   (the final assistant text)
//!   Agent::tools(&self) -> &Session<O>
//!   AgentError::{Connect(String), Tls(String), Http(u16, String), Api(String), Protocol(String), TooManyTurns}
//!
//! Semantics: one TLS connection per request (`connection: close`); request headers `host`
//! (the server name), `x-api-key`, `anthropic-version: 2023-06-01`, `content-type:
//! application/json`, `content-length`; the body is otter-claude's request with every tool of
//! `otter_tools::TOOLS` (in order, schemas verbatim). Tool calls go through the Session (which
//! validates, applies the D30 policy, asks the Os for approval and logs); the agent adds no
//! approval of its own. Conversation history persists across `ask` calls. A non-200 response with
//! a JSON error body is `Api(message)`; a 200 stream that ends before `message_stop` is `Protocol`;
//! any certificate problem is `Tls` and no HTTP request is sent.

use otter_agent::{Agent, AgentError, Backend, ByteStream, Connector};
use otter_tools::{App, Approval, CommandOutput, DirEntry, FetchOutput, Os, OsError, Outcome, Session};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const KEY: &str = "sk-ant-test-0000";
const ROOT_DER: &[u8] = include_bytes!("../../otter-x509/tests/fixtures/pki/root.der");
const NOW: u64 = 1_790_000_000; // 2026-09, inside the test PKI's 2024..2034 validity
const TOOLS: &str = r#"["list_dir","read_file","write_file","run_command","open_app","fetch_url","screenshot"]"#;

// ---------------------------------------------------------------------------------------------
// Test plumbing: a std TCP connector, a deterministic RNG, an in-memory OS, the mock server.
// ---------------------------------------------------------------------------------------------

struct Tcp(TcpStream);

impl ByteStream for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        self.0.read(buf).map_err(|e| e.to_string())
    }
    fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        Write::write_all(&mut self.0, data).map_err(|e| e.to_string())
    }
}

struct StdConnector;

impl Connector for StdConnector {
    fn connect(&mut self, host: &str, port: u16) -> Result<Box<dyn ByteStream>, String> {
        let s = TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
        Ok(Box::new(Tcp(s)))
    }
}

struct TestRng(u64);

impl otter_tls::Rng for TestRng {
    fn fill(&mut self, dest: &mut [u8]) {
        for b in dest {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            *b = (self.0 >> 24) as u8;
        }
    }
}

#[derive(Default)]
struct MemOs {
    files: BTreeMap<String, Vec<u8>>,
    answer: bool,
    asked: Vec<Approval>,
    opened: Vec<(App, Option<String>)>,
}

impl Os for MemOs {
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, OsError> {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        Ok(self
            .files
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|n| DirEntry { name: n.into(), is_dir: false, size: v.len() as u64 }))
            .collect())
    }
    fn read_file(&mut self, path: &str, limit: usize) -> Result<(Vec<u8>, u64), OsError> {
        let d = self.files.get(path).ok_or(OsError::NotFound)?;
        Ok((d[..d.len().min(limit)].to_vec(), d.len() as u64))
    }
    fn file_size(&mut self, path: &str) -> Result<Option<u64>, OsError> {
        Ok(self.files.get(path).map(|d| d.len() as u64))
    }
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), OsError> {
        self.files.insert(path.into(), data.to_vec());
        Ok(())
    }
    fn run_command(&mut self, _c: &str, _t: u64, _l: usize) -> Result<CommandOutput, OsError> {
        Err(OsError::Io("no shell in this test".into()))
    }
    fn open_app(&mut self, app: App, path: Option<&str>) -> Result<(), OsError> {
        self.opened.push((app, path.map(String::from)));
        Ok(())
    }
    fn fetch(&mut self, _url: &str, _limit: usize) -> Result<FetchOutput, OsError> {
        Err(OsError::Io("no network in this test".into()))
    }
    fn screenshot(&mut self) -> Result<Vec<u8>, OsError> {
        Ok(b"\x89PNG\r\n\x1a\n".to_vec())
    }
    fn ask(&mut self, approval: &Approval) -> bool {
        self.asked.push(approval.clone());
        self.answer
    }
}

struct Mock {
    child: Child,
    port: u16,
    log: PathBuf,
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Mock {
    fn requests(&self) -> Vec<otter_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|l| otter_json::parse_str(l).expect("the mock logs JSON lines"))
            .collect()
    }
}

fn mock(name: &str, turns: &str) -> Mock {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("otter-agent-oracle-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let scenario = dir.join("scenario.json");
    std::fs::write(&scenario, format!(r#"{{"api_key": "{KEY}", "tools": {TOOLS}, "turns": {turns}}}"#)).unwrap();
    let log = dir.join("requests.jsonl");
    let mut child = Command::new("python3")
        .arg(root.join("scripts/mock_claude.py"))
        .args(["--scenario", scenario.to_str().unwrap(), "--log", log.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port = line.trim().strip_prefix("LISTENING ").expect("LISTENING <port>").parse().unwrap();
    Mock { child, port, log }
}

fn backend(port: u16) -> Backend {
    Backend {
        host: "127.0.0.1".into(),
        port,
        server_name: "leaf.otter-test.example".into(),
        path: "/v1/messages".into(),
        api_key: KEY.into(),
        roots_der: vec![ROOT_DER.to_vec()],
        now_unix: NOW,
    }
}

fn agent(port: u16, answer: bool) -> Agent<MemOs> {
    let os = MemOs { answer, ..Default::default() };
    let session = Session::new(os).with_secret(KEY.as_bytes());
    Agent::new(backend(port), Box::new(StdConnector), Box::new(TestRng(0x2545_F491_4F6C_DD1D)), session, "You operate OtterOS.")
}

fn errors(m: &Mock) -> Vec<String> {
    m.requests()
        .iter()
        .filter_map(|r| r.get("error").and_then(|e| e.as_str()).map(String::from))
        .collect()
}

// ---------------------------------------------------------------------------------------------

const NOTE_TURNS: &str = r#"[
  {"expect": {"last_user_text": "Make a note that otters hold hands, then open it.", "message_count": 1},
   "reply": [{"thinking": "Write the note first.", "signature": "sig-1"},
             {"text": "I'll write the note."},
             {"tool_use": {"id": "toolu_01", "name": "write_file",
                           "input": {"path": "/data/notes/otters.txt", "content": "Otters hold hands while they sleep."}}}],
   "stop_reason": "tool_use"},
  {"expect": {"echo_signature": "sig-1", "message_count": 3,
              "tool_result": {"tool_use_id": "toolu_01", "content": "Wrote 35 bytes to /data/notes/otters.txt", "is_error": false}},
   "reply": [{"tool_use": {"id": "toolu_02", "name": "open_app", "input": {"app": "editor", "path": "/data/notes/otters.txt"}}}],
   "stop_reason": "tool_use"},
  {"expect": {"message_count": 5,
              "tool_result": {"tool_use_id": "toolu_02", "content": "Opened /data/notes/otters.txt in Editor", "is_error": false}},
   "reply": [{"text": "Done: the note is open in the editor."}],
   "stop_reason": "end_turn"}
]"#;

#[test]
fn writes_a_note_and_opens_it() {
    let m = mock("note", NOTE_TURNS);
    let mut a = agent(m.port, true);
    let reply = a.ask("Make a note that otters hold hands, then open it.");
    assert_eq!(errors(&m), Vec::<String>::new(), "the mock rejected a request");
    assert_eq!(reply.as_deref(), Ok("Done: the note is open in the editor."));
    let os = a.tools().os();
    assert_eq!(os.files.get("/data/notes/otters.txt").map(|v| v.as_slice()), Some(&b"Otters hold hands while they sleep."[..]));
    let asked: Vec<&str> = os.asked.iter().map(|x| x.summary.as_str()).collect();
    assert_eq!(asked, ["Write 35 bytes to /data/notes/otters.txt", "Open /data/notes/otters.txt in Editor"]);
    assert_eq!(os.opened, [(App::Editor, Some("/data/notes/otters.txt".into()))]);
    let outcomes: Vec<Outcome> = a.tools().log().iter().map(|e| e.outcome.clone()).collect();
    assert_eq!(outcomes, [Outcome::Allowed, Outcome::Allowed]);
    let reqs = m.requests();
    assert_eq!(reqs.len(), 3);
    for r in &reqs {
        let h = r.get("headers").unwrap();
        assert_eq!(h.get("x-api-key").and_then(|v| v.as_str()), Some("match"));
        assert_eq!(h.get("anthropic-version").and_then(|v| v.as_str()), Some("2023-06-01"));
        assert_eq!(h.get("host").and_then(|v| v.as_str()), Some("leaf.otter-test.example"));
    }
}

#[test]
fn a_denied_write_is_reported_to_claude() {
    let turns = r#"[
      {"expect": {"last_user_text": "Save a note."},
       "reply": [{"tool_use": {"id": "toolu_09", "name": "write_file", "input": {"path": "/data/n.txt", "content": "x"}}}],
       "stop_reason": "tool_use"},
      {"expect": {"tool_result": {"tool_use_id": "toolu_09", "content": "The user denied this action.", "is_error": true}},
       "reply": [{"text": "Understood; nothing was written."}], "stop_reason": "end_turn"}
    ]"#;
    let m = mock("deny", turns);
    let mut a = agent(m.port, false);
    assert_eq!(a.ask("Save a note."), Ok("Understood; nothing was written.".into()));
    assert!(errors(&m).is_empty(), "{:?}", errors(&m));
    assert!(a.tools().os().files.is_empty());
    assert_eq!(a.tools().log()[0].outcome, Outcome::Denied);
}

#[test]
fn refused_paths_never_reach_the_dialog() {
    let turns = r#"[
      {"expect": {"last_user_text": "What is my API key?"},
       "reply": [{"tool_use": {"id": "toolu_07", "name": "read_file", "input": {"path": "/data/config/anthropic.key"}}}],
       "stop_reason": "tool_use"},
      {"expect": {"tool_result": {"tool_use_id": "toolu_07", "content_prefix": "refused", "is_error": true}},
       "reply": [{"text": "I can't read that file."}], "stop_reason": "end_turn"}
    ]"#;
    let m = mock("refuse", turns);
    let mut a = agent(m.port, true);
    assert_eq!(a.ask("What is my API key?"), Ok("I can't read that file.".into()));
    assert!(errors(&m).is_empty(), "{:?}", errors(&m));
    assert!(a.tools().os().asked.is_empty());
    assert_eq!(a.tools().log()[0].outcome, Outcome::Refused);
}

#[test]
fn history_persists_across_questions() {
    let turns = r#"[
      {"expect": {"last_user_text": "Hello", "message_count": 1}, "reply": [{"text": "Hi!"}], "stop_reason": "end_turn"},
      {"expect": {"last_user_text": "Thanks", "message_count": 3}, "reply": [{"text": "You're welcome."}], "stop_reason": "end_turn"}
    ]"#;
    let m = mock("history", turns);
    let mut a = agent(m.port, true);
    assert_eq!(a.ask("Hello"), Ok("Hi!".into()));
    assert_eq!(a.ask("Thanks"), Ok("You're welcome.".into()));
    assert!(errors(&m).is_empty(), "{:?}", errors(&m));
}

#[test]
fn api_errors_surface() {
    let turns = r#"[{"expect": {}, "http_error": {"status": 529, "type": "overloaded_error", "message": "Overloaded"}}]"#;
    let m = mock("overloaded", turns);
    let mut a = agent(m.port, true);
    match a.ask("Hello") {
        Err(AgentError::Api(msg)) => assert!(msg.contains("Overloaded"), "{msg}"),
        other => panic!("expected Api(Overloaded), got {other:?}"),
    }
}

#[test]
fn a_cut_stream_is_an_error_not_a_hang() {
    let turns = r#"[{"expect": {}, "reply": [{"text": "This answer will be cut off in the middle of the stream."}],
                     "stop_reason": "end_turn", "close_after_bytes": 300}]"#;
    let m = mock("cut", turns);
    let mut a = agent(m.port, true);
    let started = std::time::Instant::now();
    let r = a.ask("Hello");
    assert!(matches!(r, Err(AgentError::Protocol(_))), "{r:?}");
    assert!(started.elapsed().as_secs() < 15, "no hang");
}

#[test]
fn a_wrong_certificate_name_is_refused_before_any_request() {
    let m = mock("name", r#"[{"expect": {}, "reply": [{"text": "unreachable"}], "stop_reason": "end_turn"}]"#);
    let mut b = backend(m.port);
    b.server_name = "api.anthropic.com".into();
    let session = Session::new(MemOs::default());
    let mut a = Agent::new(b, Box::new(StdConnector), Box::new(TestRng(7)), session, "x");
    assert!(matches!(a.ask("Hello"), Err(AgentError::Tls(_))));
    assert!(m.requests().is_empty(), "the key was never sent");
}

#[test]
fn an_untrusted_root_is_refused_before_any_request() {
    let m = mock("root", r#"[{"expect": {}, "reply": [{"text": "unreachable"}], "stop_reason": "end_turn"}]"#);
    let mut b = backend(m.port);
    b.roots_der = vec![];
    let session = Session::new(MemOs::default());
    let mut a = Agent::new(b, Box::new(StdConnector), Box::new(TestRng(9)), session, "x");
    assert!(matches!(a.ask("Hello"), Err(AgentError::Tls(_))));
    assert!(m.requests().is_empty());
}

#[test]
fn nothing_listening_is_a_connect_error() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut a = agent(port, true);
    assert!(matches!(a.ask("Hello"), Err(AgentError::Connect(_))));
}
