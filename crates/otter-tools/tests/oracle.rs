//! Acceptance oracle for brief M9-T0d (otter-tools: the agent's tools, their safety policy and the
//! action log), written by the orchestrator. The crate must pass this file unchanged.
//!
//! The file tree the agent sees (briefs M3-T3, D23, D30): `/` is a ramfs, `/data` is the FAT32 data
//! partition, `/dev` holds devices, `/tmp` is scratch. `/data/config` holds the Claude API key and
//! is never touched by a tool. FAT compares names case-insensitively, so every policy does too.
//!
//! API this file relies on:
//!   TOOLS: [ToolSpec; 7], ToolSpec { name, description, input_schema (JSON text), needs_approval }
//!   trait Os {
//!     fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, OsError>;
//!     fn read_file(&mut self, path: &str, limit: usize) -> Result<(Vec<u8>, u64), OsError>; // up to `limit` bytes + total size
//!     fn file_size(&mut self, path: &str) -> Result<Option<u64>, OsError>;                    // None when absent
//!     fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), OsError>;
//!     fn run_command(&mut self, command: &str, timeout_ms: u64, limit: usize) -> Result<CommandOutput, OsError>;
//!     fn open_app(&mut self, app: App, path: Option<&str>) -> Result<(), OsError>;
//!     fn fetch(&mut self, url: &str, limit: usize) -> Result<FetchOutput, OsError>;
//!     fn screenshot(&mut self) -> Result<Vec<u8>, OsError>;                                   // PNG bytes
//!     fn ask(&mut self, approval: &Approval) -> bool;                                        // the Allow/Deny dialog
//!   }
//!   DirEntry { name: String, is_dir: bool, size: u64 }; CommandOutput { status: i32, output: Vec<u8>, truncated: bool }
//!   FetchOutput { status: u16, content_type: String, body: Vec<u8>, truncated: bool }
//!   App::{Terminal, Editor, Files, Otter}; OsError::{NotFound, NotADirectory, IsADirectory, NoSpace, Timeout, Io(String)}
//!   Approval { tool: String, summary: String, detail: String }
//!   Session::new(os) -> Session<O>; .with_secret(&[u8]) -> Self; .call(tool, input_json) -> ToolResult;
//!     .log() -> &[LogEntry]; .os() -> &O; .os_mut() -> &mut O
//!   ToolResult { content: Vec<Content>, is_error: bool }; Content::{Text(String), Png(Vec<u8>)}
//!   LogEntry { tool: String, summary: String, outcome: Outcome }
//!   Outcome::{AutoRun, Allowed, Denied, Refused, Invalid, Failed}
//!   normalize(path) -> Result<String, PathError>
//!   (all public types derive Debug, Clone, PartialEq, Eq; App also Copy)
//!
//! Policy, in the order it is applied to a call:
//! 1. Unknown tool, or input that is not a JSON object with exactly the schema's properties (every
//!    required one, no others, the right JSON types, enum values from the schema): Invalid, error
//!    text starting "invalid input", nothing asked or run.
//! 2. Paths are normalised: absolute, `.`/`..`/repeated slashes resolved (`..` at `/` stays at `/`),
//!    no trailing slash, components of 1-255 bytes without `\ : * ? " < > |` or control characters
//!    and not ending in `.` or a space, at most 1024 bytes. A path that fails: Invalid.
//! 3. Protected (Refused, error text starting "refused"): `/data/config` and everything under it, and
//!    `/dev` and everything under it, for every tool; writes outside `/data` and `/tmp`; open_app
//!    paths under the protected trees; run_command text containing "anthropic.key" or "/config"
//!    (ASCII case-insensitive); fetch URLs that are not http:// or https:// with a host, or longer
//!    than 2048 bytes (those are Invalid, not Refused).
//! 4. needs_approval tools (write_file, run_command, open_app, fetch_url) ask the Os with an Approval
//!    whose summary is exactly as tested below; a "no" is Denied (error "The user denied this
//!    action."). list_dir, read_file and screenshot are AutoRun.
//! 5. Os errors are Failed (error text starting "failed"). Otherwise the outcome is AutoRun/Allowed.
//!
//! Every call, whatever its outcome, appends one LogEntry (the summary is the approval summary for
//! approval tools, "List /data", "Read /data/a.txt" and "Screenshot" for the others, and the tool
//! name for invalid input). Output formats are pinned by the tests: list_dir sorts names ASCII
//! case-insensitively (ties by byte order), directories as "name/", files as "name (N bytes)";
//! read_file returns UTF-8 text (a character cut by the 64 KiB limit is dropped) or "[binary file, N
//! bytes]"; fetch_url shows the body as text when the content type is text/*, application/json,
//! application/xml, application/javascript or ends in +json/+xml and the body is UTF-8, else
//! "[binary body]"; failures read "failed: " + the error (Io carries its own message). With a
//! secret set, every occurrence of it in any text returned to the model, shown in a dialog or
//! stored in the log becomes "[REDACTED]".

use otter_tools::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------------
// A scripted Os.
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct MockOs {
    files: BTreeMap<String, Vec<u8>>,
    dirs: Vec<String>,
    answer: bool,
    asked: Vec<Approval>,
    ran: Vec<String>,
    opened: Vec<(App, Option<String>)>,
    fetched: Vec<String>,
    command: Option<CommandOutput>,
    page: Option<FetchOutput>,
    calls: usize,
}

impl Os for MockOs {
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, OsError> {
        self.calls += 1;
        if !self.dirs.iter().any(|d| d == path) {
            return Err(if self.files.contains_key(path) { OsError::NotADirectory } else { OsError::NotFound });
        }
        let prefix = if path == "/" { "/".to_string() } else { format!("{path}/") };
        let mut out = Vec::new();
        for (name, data) in &self.files {
            if let Some(rest) = name.strip_prefix(&prefix)
                && !rest.contains('/')
            {
                out.push(DirEntry { name: rest.into(), is_dir: false, size: data.len() as u64 });
            }
        }
        for d in &self.dirs {
            if let Some(rest) = d.strip_prefix(&prefix)
                && !rest.is_empty()
                && !rest.contains('/')
            {
                out.push(DirEntry { name: rest.into(), is_dir: true, size: 0 });
            }
        }
        out.reverse(); // the tool must sort
        Ok(out)
    }
    fn read_file(&mut self, path: &str, limit: usize) -> Result<(Vec<u8>, u64), OsError> {
        self.calls += 1;
        if self.dirs.iter().any(|d| d == path) {
            return Err(OsError::IsADirectory);
        }
        let data = self.files.get(path).ok_or(OsError::NotFound)?;
        Ok((data[..data.len().min(limit)].to_vec(), data.len() as u64))
    }
    fn file_size(&mut self, path: &str) -> Result<Option<u64>, OsError> {
        Ok(self.files.get(path).map(|d| d.len() as u64))
    }
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), OsError> {
        self.calls += 1;
        if path.starts_with("/data/full") {
            return Err(OsError::NoSpace);
        }
        self.files.insert(path.into(), data.to_vec());
        Ok(())
    }
    fn run_command(&mut self, command: &str, timeout_ms: u64, limit: usize) -> Result<CommandOutput, OsError> {
        self.calls += 1;
        assert_eq!((timeout_ms, limit), (30_000, 16_384), "commands run with a 30 s timeout and 16 KiB of output");
        self.ran.push(command.into());
        self.command.clone().ok_or(OsError::Timeout)
    }
    fn open_app(&mut self, app: App, path: Option<&str>) -> Result<(), OsError> {
        self.calls += 1;
        self.opened.push((app, path.map(String::from)));
        Ok(())
    }
    fn fetch(&mut self, url: &str, limit: usize) -> Result<FetchOutput, OsError> {
        self.calls += 1;
        assert_eq!(limit, 65_536);
        self.fetched.push(url.into());
        self.page.clone().ok_or(OsError::Io("connection refused".into()))
    }
    fn screenshot(&mut self) -> Result<Vec<u8>, OsError> {
        self.calls += 1;
        Ok(b"\x89PNG\r\n\x1a\nfake".to_vec())
    }
    fn ask(&mut self, approval: &Approval) -> bool {
        self.asked.push(approval.clone());
        self.answer
    }
}

fn session() -> Session<MockOs> {
    let mut os = MockOs { answer: true, ..Default::default() };
    os.dirs = vec!["/".into(), "/data".into(), "/data/notes".into(), "/data/config".into(), "/tmp".into(), "/data/empty".into()];
    os.files.insert("/data/notes/today.txt".into(), b"otters hold hands\n".to_vec());
    os.files.insert("/data/Notes.md".into(), b"# Notes\n".to_vec());
    os.files.insert("/data/config/anthropic.key".into(), b"sk-ant-test-0000".to_vec());
    os.files.insert("/data/photo.png".into(), vec![0x89, b'P', b'N', b'G', 0, 1, 2, 0xFF]);
    Session::new(os)
}

fn text(r: &ToolResult) -> String {
    r.content
        .iter()
        .map(|c| match c {
            Content::Text(t) => t.clone(),
            Content::Png(_) => "<png>".into(),
        })
        .collect::<Vec<_>>()
        .join("")
}

fn last_outcome(s: &Session<MockOs>) -> (String, Outcome) {
    let e = s.log().last().expect("a log entry");
    (e.summary.clone(), e.outcome.clone())
}

// ---------------------------------------------------------------------------------------------

#[test]
fn tool_specs() {
    let names: Vec<(&str, bool)> = TOOLS.iter().map(|t| (t.name, t.needs_approval)).collect();
    assert_eq!(
        names,
        [
            ("list_dir", false),
            ("read_file", false),
            ("write_file", true),
            ("run_command", true),
            ("open_app", true),
            ("fetch_url", true),
            ("screenshot", false)
        ]
    );
    for t in &TOOLS {
        let schema = otter_json::parse_str(t.input_schema).unwrap_or_else(|e| panic!("{}: schema is not JSON: {e:?}", t.name));
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"), "{}", t.name);
        assert!(schema.get("properties").is_some(), "{}", t.name);
        assert_eq!(schema.get("additionalProperties").and_then(|v| v.as_bool()), Some(false), "{}", t.name);
        assert!(t.description.len() >= 40, "{}: describe the tool for the model", t.name);
    }
    let open = TOOLS.iter().find(|t| t.name == "open_app").unwrap();
    for app in ["terminal", "editor", "files", "otter"] {
        assert!(open.input_schema.contains(&format!("\"{app}\"")), "open_app's schema enumerates {app}");
    }
}

#[test]
fn path_normalisation() {
    let ok = [
        ("/", "/"),
        ("/data", "/data"),
        ("/data/", "/data"),
        ("//data///notes//", "/data/notes"),
        ("/data/./notes/../notes/today.txt", "/data/notes/today.txt"),
        ("/../../data", "/data"),
        ("/data/..", "/"),
        ("/data/résumé de juillet.txt", "/data/résumé de juillet.txt"),
    ];
    for (input, expected) in ok {
        assert_eq!(normalize(input).as_deref(), Ok(expected), "{input:?}");
    }
    let long_component = format!("/data/{}", "a".repeat(256));
    let long_path = format!("/data/{}", vec!["b".repeat(200); 6].join("/")); // 1211 bytes
    for bad in ["", "data/notes", "relative", "/data/a:b", "/data/a*b", "/data/a?b", "/data/a\"b", "/data/a<b", "/data/a>b", "/data/a|b", "/data/a\\b", "/data/tab\there", "/data/dot.", "/data/space ", long_component.as_str(), long_path.as_str()] {
        assert!(normalize(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn list_dir_sorts_and_formats() {
    let mut s = session();
    let r = s.call("list_dir", r#"{"path": "/data/"}"#);
    assert!(!r.is_error);
    assert_eq!(text(&r), "config/\nempty/\nnotes/\nNotes.md (8 bytes)\nphoto.png (8 bytes)");
    assert_eq!(last_outcome(&s), ("List /data".into(), Outcome::AutoRun));
    let r = s.call("list_dir", r#"{"path": "/data/empty"}"#);
    assert_eq!(text(&r), "(empty directory)");
    let r = s.call("list_dir", r#"{"path": "/data/nope"}"#);
    assert!(r.is_error && text(&r).starts_with("failed"), "{}", text(&r));
    assert_eq!(last_outcome(&s).1, Outcome::Failed);
    assert!(s.os().asked.is_empty(), "read-only tools never ask");
}

#[test]
fn read_file_text_binary_and_truncation() {
    let mut s = session();
    let r = s.call("read_file", r#"{"path": "/data/notes/../notes/today.txt"}"#);
    assert_eq!((text(&r).as_str(), r.is_error), ("otters hold hands\n", false));
    assert_eq!(last_outcome(&s), ("Read /data/notes/today.txt".into(), Outcome::AutoRun));
    let r = s.call("read_file", r#"{"path": "/data/photo.png"}"#);
    assert_eq!(text(&r), "[binary file, 8 bytes]");
    let big: String = (0..70_000).map(|i| if i % 64 == 63 { '\n' } else { 'é' }).collect(); // 2-byte chars
    s.os_mut().files.insert("/data/big.txt".into(), big.clone().into_bytes());
    let r = s.call("read_file", r#"{"path": "/data/big.txt"}"#);
    let t = text(&r);
    let note = format!("\n[truncated: showing 65536 of {} bytes]", big.len());
    let tail: String = t.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
    assert!(t.ends_with(&note), "{tail}");
    let shown = &t[..t.len() - note.len()];
    assert!(shown.len() <= 65_536 && shown.len() >= 65_535 && big.starts_with(shown), "cut at a character boundary");
    let r = s.call("read_file", r#"{"path": "/data/notes"}"#);
    assert!(r.is_error && text(&r).starts_with("failed"));
}

#[test]
fn the_key_and_devices_are_never_touched() {
    let mut s = session();
    let attempts = [
        ("read_file", r#"{"path": "/data/config/anthropic.key"}"#),
        ("read_file", r#"{"path": "/DATA/Config/ANTHROPIC.KEY"}"#),
        ("read_file", r#"{"path": "/data/notes/../config/anthropic.key"}"#),
        ("list_dir", r#"{"path": "/data/config"}"#),
        ("list_dir", r#"{"path": "/data/CONFIG/"}"#),
        ("write_file", r#"{"path": "/data/config/anthropic.key", "content": "x"}"#),
        ("open_app", r#"{"app": "editor", "path": "/data/config/anthropic.key"}"#),
        ("read_file", r#"{"path": "/dev/fb0"}"#),
        ("list_dir", r#"{"path": "/dev"}"#),
        ("run_command", r#"{"command": "cat /data/config/anthropic.key"}"#),
        ("run_command", r#"{"command": "cp /data/CONFIG/x /tmp"}"#),
        ("run_command", r#"{"command": "cat /data/Anthropic.Key"}"#),
    ];
    for (tool, input) in attempts {
        let r = s.call(tool, input);
        assert!(r.is_error && text(&r).starts_with("refused"), "{tool} {input}: {}", text(&r));
        assert_eq!(last_outcome(&s).1, Outcome::Refused, "{tool} {input}");
    }
    assert!(s.os().asked.is_empty(), "refusals happen before any dialog");
    assert_eq!(s.os().calls, 0, "and before the Os is touched");
    let r = s.call("list_dir", r#"{"path": "/data"}"#);
    assert!(text(&r).contains("config/"), "listing /data itself is fine");
    let r = s.call("read_file", r#"{"path": "/data/configs.txt"}"#);
    assert_eq!(last_outcome(&s).1, Outcome::Failed, "a name that merely starts with 'config' is not protected: {}", text(&r));
    let r = s.call("read_file", r#"{"path": "/device-notes.txt"}"#);
    assert_eq!(last_outcome(&s).1, Outcome::Failed, "nor one that starts with 'dev': {}", text(&r));
}

#[test]
fn writes_only_under_data_and_tmp() {
    let mut s = session();
    for path in ["/bin/sh", "/etc/motd", "/", "/datastore/x", "/data"] {
        let r = s.call("write_file", &format!(r#"{{"path": "{path}", "content": "x"}}"#));
        assert!(r.is_error && text(&r).starts_with("refused"), "{path}: {}", text(&r));
    }
    let r = s.call("write_file", r#"{"path": "/tmp/scratch.txt", "content": "hi"}"#);
    assert_eq!((text(&r).as_str(), r.is_error), ("Wrote 2 bytes to /tmp/scratch.txt", false));
    assert!(s.os().asked.len() == 1, "the allowed write was approved first");
}

#[test]
fn approvals_and_the_action_log() {
    let mut s = session();
    let r = s.call("write_file", r#"{"path": "/data/notes/idea.txt", "content": "an OS by an AI"}"#);
    assert_eq!(text(&r), "Wrote 14 bytes to /data/notes/idea.txt");
    let r = s.call("write_file", r#"{"path": "/data/notes/today.txt", "content": "otters float"}"#);
    assert_eq!(text(&r), "Wrote 12 bytes to /data/notes/today.txt");
    let asked: Vec<(String, String, String)> = s.os().asked.iter().map(|a| (a.tool.clone(), a.summary.clone(), a.detail.clone())).collect();
    assert_eq!(
        asked,
        [
            ("write_file".into(), "Write 14 bytes to /data/notes/idea.txt".into(), "an OS by an AI".into()),
            ("write_file".into(), "Replace /data/notes/today.txt (18 bytes) with 12 bytes".into(), "otters float".into()),
        ]
    );
    s.os_mut().answer = false;
    let r = s.call("write_file", r#"{"path": "/data/x.txt", "content": "no"}"#);
    assert_eq!((text(&r).as_str(), r.is_error), ("The user denied this action.", true));
    assert!(!s.os().files.contains_key("/data/x.txt"), "a denied write does not happen");
    s.os_mut().answer = true;
    let r = s.call("write_file", r#"{"path": "/data/full/x.txt", "content": "1234"}"#);
    assert!(r.is_error && text(&r).starts_with("failed"));
    let r = s.call("open_app", r#"{"app": "editor", "path": "/data/notes/idea.txt"}"#);
    assert_eq!((text(&r).as_str(), r.is_error), ("Opened /data/notes/idea.txt in Editor", false));
    let r = s.call("open_app", r#"{"app": "terminal"}"#);
    assert_eq!(text(&r), "Opened Terminal");
    assert_eq!(s.os().opened, [(App::Editor, Some("/data/notes/idea.txt".into())), (App::Terminal, None)]);
    let log: Vec<(&str, &str, Outcome)> = s.log().iter().map(|e| (e.tool.as_str(), e.summary.as_str(), e.outcome.clone())).collect();
    assert_eq!(
        log,
        [
            ("write_file", "Write 14 bytes to /data/notes/idea.txt", Outcome::Allowed),
            ("write_file", "Replace /data/notes/today.txt (18 bytes) with 12 bytes", Outcome::Allowed),
            ("write_file", "Write 2 bytes to /data/x.txt", Outcome::Denied),
            ("write_file", "Write 4 bytes to /data/full/x.txt", Outcome::Failed),
            ("open_app", "Open /data/notes/idea.txt in Editor", Outcome::Allowed),
            ("open_app", "Open Terminal", Outcome::Allowed),
        ]
    );
}

#[test]
fn approval_details_are_bounded() {
    let mut s = session();
    let content = "z".repeat(5000);
    s.call("write_file", &format!(r#"{{"path": "/data/z.txt", "content": "{content}"}}"#));
    let detail = &s.os().asked[0].detail;
    assert!(detail.starts_with(&"z".repeat(2048)) && detail.ends_with("\n[... 2952 more bytes]"), "the dialog previews 2 KiB");
    let long_command = format!("echo {}", "w".repeat(300));
    s.os_mut().command = Some(CommandOutput { status: 0, output: b"ok".to_vec(), truncated: false });
    s.call("run_command", &format!(r#"{{"command": "{long_command}"}}"#));
    let a = &s.os().asked[1];
    assert_eq!(a.summary, format!("Run: {}…", &long_command[..200]), "summaries show at most 200 characters");
    assert_eq!(a.detail, long_command, "the detail shows the whole command");
}

#[test]
fn run_command_and_fetch() {
    let mut s = session();
    s.os_mut().command = Some(CommandOutput { status: 0, output: b"hello\nworld\n".to_vec(), truncated: false });
    let r = s.call("run_command", r#"{"command": "ls /data"}"#);
    assert_eq!(text(&r), "exit status 0\nhello\nworld\n");
    assert_eq!(s.os().asked[0].summary, "Run: ls /data");
    s.os_mut().command = Some(CommandOutput { status: 3, output: vec![b'x'; 16_384], truncated: true });
    let r = s.call("run_command", r#"{"command": "yes x"}"#);
    let t = text(&r);
    assert!(t.starts_with("exit status 3\nxxx") && t.ends_with("\n[output truncated at 16384 bytes]"));
    assert!(!r.is_error, "a nonzero exit status is a result, not a tool error");
    s.os_mut().command = None;
    let r = s.call("run_command", r#"{"command": "sleep 99"}"#);
    assert!(r.is_error && text(&r).starts_with("failed"), "timeouts are failures");

    s.os_mut().page = Some(FetchOutput { status: 200, content_type: "text/html; charset=utf-8".into(), body: b"<title>Example Domain</title>".to_vec(), truncated: false });
    let r = s.call("fetch_url", r#"{"url": "https://example.com/"}"#);
    assert_eq!(text(&r), "HTTP 200, text/html; charset=utf-8, 29 bytes\n\n<title>Example Domain</title>");
    assert_eq!(s.os().asked.last().unwrap().summary, "Fetch https://example.com/");
    s.os_mut().page = Some(FetchOutput { status: 200, content_type: "image/png".into(), body: vec![0x89, 0x50], truncated: false });
    let r = s.call("fetch_url", r#"{"url": "http://example.com/a.png"}"#);
    assert_eq!(text(&r), "HTTP 200, image/png, 2 bytes\n\n[binary body]");
    s.os_mut().page = None;
    let r = s.call("fetch_url", r#"{"url": "https://down.example/"}"#);
    assert!(r.is_error && text(&r).starts_with("failed") && text(&r).contains("connection refused"));
    let fetched_before = s.os().fetched.len();
    let long_url = format!("https://example.com/{}", "a".repeat(2040));
    for url in ["file:///data/config/anthropic.key", "ftp://example.com/", "https://", "example.com", "javascript:alert(1)", long_url.as_str()] {
        let r = s.call("fetch_url", &format!(r#"{{"url": "{url}"}}"#));
        assert!(r.is_error && text(&r).starts_with("invalid input"), "{url}: {}", text(&r));
    }
    assert_eq!(s.os().fetched.len(), fetched_before, "bad URLs are never fetched");
}

#[test]
fn screenshot_returns_an_image() {
    let mut s = session();
    let r = s.call("screenshot", "{}");
    assert_eq!(r.content, [Content::Png(b"\x89PNG\r\n\x1a\nfake".to_vec())]);
    assert_eq!(last_outcome(&s), ("Screenshot".into(), Outcome::AutoRun));
}

#[test]
fn invalid_input_is_rejected_before_anything_runs() {
    let mut s = session();
    let cases = [
        ("launch_missiles", "{}"),
        ("read_file", "not json"),
        ("read_file", "[]"),
        ("read_file", "{}"),
        ("read_file", r#"{"path": 7}"#),
        ("read_file", r#"{"path": "/data/a.txt", "extra": true}"#),
        ("write_file", r#"{"path": "/data/a.txt"}"#),
        ("open_app", r#"{"app": "browser"}"#),
        ("open_app", r#"{"app": "editor", "path": null}"#),
        ("screenshot", r#"{"region": "all"}"#),
        ("list_dir", r#"{"path": "data"}"#),
        ("list_dir", r#"{"path": "/data/a:b"}"#),
    ];
    for (tool, input) in cases {
        let r = s.call(tool, input);
        assert!(r.is_error && text(&r).starts_with("invalid input"), "{tool} {input}: {}", text(&r));
        assert_eq!(s.log().last().unwrap().outcome, Outcome::Invalid, "{tool} {input}");
    }
    assert_eq!((s.os().calls, s.os().asked.len()), (0, 0));
}

#[test]
fn the_secret_never_leaves() {
    let mut s = session().with_secret(b"sk-ant-test-0000");
    s.os_mut().files.insert("/data/leak.txt".into(), b"key=sk-ant-test-0000;".to_vec());
    let r = s.call("read_file", r#"{"path": "/data/leak.txt"}"#);
    assert_eq!(text(&r), "key=[REDACTED];");
    s.os_mut().command = Some(CommandOutput { status: 0, output: b"sk-ant-test-0000sk-ant-test-0000".to_vec(), truncated: false });
    let r = s.call("run_command", r#"{"command": "env"}"#);
    assert_eq!(text(&r), "exit status 0\n[REDACTED][REDACTED]");
    let r = s.call("write_file", r#"{"path": "/data/w.txt", "content": "sk-ant-test-0000"}"#);
    assert!(!r.is_error);
    assert!(s.os().asked.iter().all(|a| !a.detail.contains("sk-ant")), "not even in the dialog");
    for e in s.log() {
        assert!(!e.summary.contains("sk-ant"), "{e:?}");
    }
}
