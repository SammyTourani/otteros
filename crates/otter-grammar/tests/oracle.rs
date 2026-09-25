//! Acceptance oracle for brief M9-T0c, written by the orchestrator. The crate must pass this file
//! unchanged; add your own tests in other files. The references are independent of the crate:
//! otter-json parses every completed call, a strict checker written here from the tool schemas and
//! otter-claude's validator accept every tool input, a brute-force mask (each token's bytes fed to
//! a clone of the matcher) equals the trie mask, and SmolLM2's own tokenizations of valid calls are
//! accepted token by token.

use otter_grammar::{Grammar, Matcher, TokenMask, Vocab};
use otter_json::Value;
use std::sync::OnceLock;
use std::time::Instant;

const TOOLS: &[(&str, &str)] = &[
    ("list_dir", r#"{"type":"object","properties":{"path":{"type":"string","maxLength":256}},"required":["path"],"additionalProperties":false}"#),
    ("read_file", r#"{"type":"object","properties":{"path":{"type":"string","maxLength":256}},"required":["path"],"additionalProperties":false}"#),
    ("write_file", r#"{"type":"object","properties":{"path":{"type":"string","maxLength":256},"content":{"type":"string","maxLength":4096}},"required":["path","content"],"additionalProperties":false}"#),
    ("run_command", r#"{"type":"object","properties":{"command":{"type":"string","maxLength":512}},"required":["command"],"additionalProperties":false}"#),
    ("open_app", r#"{"type":"object","properties":{"app":{"type":"string","enum":["terminal","editor","files","otter"]},"path":{"type":"string","maxLength":256}},"required":["app"],"additionalProperties":false}"#),
    ("fetch_url", r#"{"type":"object","properties":{"url":{"type":"string","maxLength":2048}},"required":["url"],"additionalProperties":false}"#),
    ("screenshot", r#"{"type":"object","properties":{},"additionalProperties":false}"#),
    ("synthetic", r#"{"type":"object","properties":{"n":{"type":"integer"},"flag":{"type":"boolean"},"nothing":{"type":"null"},"tags":{"type":"array","items":{"type":"string","maxLength":16},"maxItems":3},"inner":{"type":"object","properties":{"k":{"type":"string","enum":["a","b"]}},"required":["k"],"additionalProperties":false}},"required":["n","tags"],"additionalProperties":false}"#),
];

fn grammar() -> Grammar {
    Grammar::tool_call(TOOLS, b"", b"").expect("the test tool set compiles")
}

/// Feeds `bytes` one at a time and returns the index of the first rejected byte.
fn first_rejection(g: &Grammar, bytes: &[u8]) -> Option<usize> {
    let mut m = Matcher::new(g);
    bytes.iter().position(|b| m.accept_bytes(std::slice::from_ref(b)).is_err())
}

fn accepts_complete(g: &Grammar, bytes: &[u8]) -> bool {
    let mut m = Matcher::new(g);
    m.accept_bytes(bytes).is_ok() && m.is_complete()
}

// ---------------------------------------------------------------------------------------------
// Deterministic randomness and a generator of valid calls written independently of the crate.

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

struct Gen<'a> {
    rng: &'a mut Rng,
    out: String,
}

impl Gen<'_> {
    /// At most one space, and only after `{ [ , :` (the grammar's spacing rule).
    fn space(&mut self) {
        if self.rng.chance(30) {
            self.out.push(' ');
        }
    }
    fn open(&mut self, c: char) {
        self.out.push(c);
        self.space();
    }
    fn comma(&mut self) {
        self.out.push(',');
        self.space();
    }
    fn key(&mut self, k: &str) {
        self.out.push('"');
        self.out.push_str(k);
        self.out.push_str("\":");
        self.space();
    }
    /// A JSON string of at most `max_chars` characters after unescaping.
    fn string(&mut self, max_chars: usize) {
        const PLAIN: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-~:?&=#%";
        let n = self.rng.below(max_chars.min(40) + 1);
        self.out.push('"');
        for _ in 0..n {
            let piece = match self.rng.below(12) {
                0 => "\\\"",
                1 => "\\\\",
                2 => "\\n",
                3 => if self.rng.chance(50) { "é" } else { "\\u00e9" },
                4 => if self.rng.chance(50) { "日" } else { "\\u65e5" },
                5 => "🦦",
                6 => if self.rng.chance(50) { "/" } else { "\\/" },
                7 => " ",
                _ => {
                    let c = PLAIN[self.rng.below(PLAIN.len())] as char;
                    self.out.push(c);
                    continue;
                }
            };
            self.out.push_str(piece);
        }
        self.out.push('"');
    }
    fn one_of(&mut self, options: &[&str]) {
        let o = options[self.rng.below(options.len())];
        self.out.push('"');
        self.out.push_str(o);
        self.out.push('"');
    }
    fn integer(&mut self) {
        if self.rng.chance(15) {
            self.out.push('0');
            return;
        }
        if self.rng.chance(40) {
            self.out.push('-');
        }
        let digits = 1 + self.rng.below(18);
        self.out.push((b'1' + self.rng.below(9) as u8) as char);
        for _ in 1..digits {
            self.out.push((b'0' + self.rng.below(10) as u8) as char);
        }
    }
}

fn gen_call(rng: &mut Rng) -> String {
    let tool = TOOLS[rng.below(TOOLS.len())].0;
    let mut g = Gen { rng, out: String::new() };
    g.open('{');
    g.key("name");
    g.one_of(&[tool]);
    g.comma();
    g.key("input");
    g.open('{');
    match tool {
        "list_dir" | "read_file" => {
            g.key("path");
            g.string(256);
        }
        "write_file" => {
            g.key("path");
            g.string(256);
            g.comma();
            g.key("content");
            g.string(4096);
        }
        "run_command" => {
            g.key("command");
            g.string(512);
        }
        "open_app" => {
            g.key("app");
            g.one_of(&["terminal", "editor", "files", "otter"]);
            if g.rng.chance(50) {
                g.comma();
                g.key("path");
                g.string(256);
            }
        }
        "fetch_url" => {
            g.key("url");
            g.string(2048);
        }
        "screenshot" => {}
        _ => {
            g.key("n");
            g.integer();
            if g.rng.chance(50) {
                g.comma();
                g.key("flag");
                let b = if g.rng.chance(50) { "true" } else { "false" };
                g.out.push_str(b);
            }
            if g.rng.chance(50) {
                g.comma();
                g.key("nothing");
                g.out.push_str("null");
            }
            g.comma();
            g.key("tags");
            g.open('[');
            for i in 0..g.rng.below(4) {
                if i > 0 {
                    g.comma();
                }
                g.string(16);
            }
            g.out.push(']');
            if g.rng.chance(50) {
                g.comma();
                g.key("inner");
                g.open('{');
                g.key("k");
                g.one_of(&["a", "b"]);
                g.out.push('}');
            }
        }
    }
    g.out.push_str("}}");
    g.out
}

/// Strict check of a parsed call against the tool set: key order, types, enums, lengths in
/// characters, item counts and integer digit counts.
fn check_call(v: &Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("top level is not an object")?;
    let keys: Vec<&str> = obj.iter().map(|(k, _)| k).collect();
    if keys != ["name", "input"] {
        return Err(format!("top-level keys {keys:?}"));
    }
    let name = obj.get("name").and_then(Value::as_str).ok_or("name is not a string")?;
    let input = obj.get("input").and_then(Value::as_object).ok_or("input is not an object")?;
    let ikeys: Vec<&str> = input.iter().map(|(k, _)| k).collect();
    let text = |k: &str, max: usize| -> Result<(), String> {
        let s = input.get(k).and_then(Value::as_str).ok_or(format!("{k} is not a string"))?;
        if s.chars().count() > max { Err(format!("{k} is longer than {max}")) } else { Ok(()) }
    };
    let keys_are = |want: &[&str]| -> Result<(), String> {
        if ikeys == want { Ok(()) } else { Err(format!("{name}: keys {ikeys:?}, expected {want:?}")) }
    };
    match name {
        "list_dir" | "read_file" => {
            keys_are(&["path"])?;
            text("path", 256)
        }
        "write_file" => {
            keys_are(&["path", "content"])?;
            text("path", 256)?;
            text("content", 4096)
        }
        "run_command" => {
            keys_are(&["command"])?;
            text("command", 512)
        }
        "open_app" => {
            if ikeys != ["app"] && ikeys != ["app", "path"] {
                return Err(format!("open_app keys {ikeys:?}"));
            }
            let app = input.get("app").and_then(Value::as_str).ok_or("app is not a string")?;
            if !["terminal", "editor", "files", "otter"].contains(&app) {
                return Err(format!("app {app:?} not in the enum"));
            }
            if input.contains_key("path") { text("path", 256) } else { Ok(()) }
        }
        "fetch_url" => {
            keys_are(&["url"])?;
            text("url", 2048)
        }
        "screenshot" => keys_are(&[]),
        "synthetic" => {
            let order = ["n", "flag", "nothing", "tags", "inner"];
            let mut pos = 0;
            for k in &ikeys {
                match order[pos..].iter().position(|o| o == k) {
                    Some(p) => pos += p + 1,
                    None => return Err(format!("synthetic key {k:?} unknown or out of order")),
                }
            }
            let n = input.get("n").and_then(Value::as_i64).ok_or("n is not an integer")?;
            if n.unsigned_abs().to_string().len() > 18 {
                return Err("n has more than 18 digits".into());
            }
            if let Some(f) = input.get("flag") {
                f.as_bool().ok_or("flag is not a boolean")?;
            }
            if let Some(x) = input.get("nothing") {
                if !x.is_null() {
                    return Err("nothing is not null".into());
                }
            }
            let tags = input.get("tags").and_then(Value::as_array).ok_or("tags is not an array")?;
            if tags.len() > 3 {
                return Err("more than 3 tags".into());
            }
            for t in tags {
                let s = t.as_str().ok_or("tag is not a string")?;
                if s.chars().count() > 16 {
                    return Err("tag longer than 16".into());
                }
            }
            if let Some(inner) = input.get("inner") {
                let io = inner.as_object().ok_or("inner is not an object")?;
                let ik: Vec<&str> = io.iter().map(|(k, _)| k).collect();
                let k = io.get("k").and_then(Value::as_str);
                if ik != ["k"] || !matches!(k, Some("a") | Some("b")) {
                    return Err(format!("inner {ik:?} {k:?}"));
                }
            }
            Ok(())
        }
        other => Err(format!("unknown tool {other:?}")),
    }
}

/// The strict checker plus otter-claude's validator for the OS tools.
fn validate(output: &[u8]) -> Result<(), String> {
    let v = otter_json::parse(output).map_err(|e| format!("not JSON ({e:?})"))?;
    check_call(&v)?;
    let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
    if name != "synthetic" {
        let schema_text = TOOLS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s).unwrap_or_default();
        let schema = otter_json::parse_str(schema_text).expect("schema parses");
        let input = otter_json::to_string(v.get("input").expect("input"));
        otter_claude::validation::validate_tool_input(&input, schema.as_object().expect("schema object"))
            .map_err(|e| format!("otter-claude validator: {e:?}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Vocabularies.

struct Real {
    vocab: Vocab,
    bytes: Vec<Vec<u8>>,
    end: u32,
    special: Vec<u32>,
    tokenizer: otter_llm::Tokenizer,
}

fn model_bytes() -> Option<&'static [u8]> {
    static BYTES: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    BYTES
        .get_or_init(|| {
            let dir = std::env::var("OTTEROS_MODEL_DIR").unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".into());
            let path = std::path::Path::new(&dir).join("SmolLM2-135M-Instruct.q8.otm");
            match std::fs::read(&path) {
                Ok(b) => Some(b),
                Err(_) if std::env::var("OTTEROS_SKIP_MODEL_TESTS").as_deref() == Ok("1") => None,
                Err(e) => panic!(
                    "SmolLM2 model missing at {} ({e}); convert it (M7) or set OTTEROS_SKIP_MODEL_TESTS=1",
                    path.display()
                ),
            }
        })
        .as_deref()
}

fn real() -> Option<Real> {
    let model = otter_llm::loader::Model::load(model_bytes()?).expect("load SmolLM2");
    let end = model
        .added_tokens
        .iter()
        .find(|(_, s)| *s == "<|im_end|>")
        .map(|(id, _)| *id)
        .expect("<|im_end|> is an added token");
    let special: Vec<u32> = model.added_tokens.iter().map(|(id, _)| *id).filter(|id| *id != end).collect();
    let bytes = model.vocab.to_vec();
    let tokenizer =
        otter_llm::Tokenizer::from_loader_data(model.vocab, model.merge_pairs, model.added_tokens, model.isolate_digits)
            .expect("tokenizer");
    let vocab = Vocab::new(bytes.clone(), end, &special);
    Some(Real { vocab, bytes, end, special, tokenizer })
}

/// All 256 single bytes, 2,000 multi-byte tokens (fragments of valid calls and structural/UTF-8
/// byte soup, including duplicates), an end token and a special token whose bytes would otherwise
/// be legal inside strings.
fn synthetic(rng: &mut Rng) -> (Vocab, Vec<Vec<u8>>, u32, Vec<u32>) {
    const SOUP: &[u8] = &[b'"', b'{', b'}', b'[', b']', b',', b':', b' ', b'\\', b'u', b'a', b'0', b'-', 0xC3, 0xA9, 0xE6, 0xFF, b'\n'];
    let mut toks: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
    while toks.len() < 256 + 2000 {
        if rng.chance(70) {
            let call = gen_call(rng).into_bytes();
            let len = 2 + rng.below(7);
            if call.len() > len {
                let start = rng.below(call.len() - len);
                toks.push(call[start..start + len].to_vec());
            }
        } else {
            let n = 2 + rng.below(3);
            toks.push((0..n).map(|_| SOUP[rng.below(SOUP.len())]).collect());
        }
    }
    toks.push(b"{\"".to_vec()); // duplicates of existing fragments must each get their own bit
    toks.push(b"{\"".to_vec());
    let end = toks.len() as u32;
    toks.push(b"<|end|>".to_vec());
    let special = toks.len() as u32;
    toks.push(b"abc".to_vec());
    let vocab = Vocab::new(toks.clone(), end, &[special]);
    (vocab, toks, end, vec![special])
}

/// The trie mask must equal the brute-force definition for every token.
fn assert_mask_matches(m: &Matcher, vocab: &Vocab, bytes: &[Vec<u8>], end: u32, special: &[u32], ctx: &str) {
    let mut mask = TokenMask::new(bytes.len());
    m.allowed_tokens(vocab, &mut mask);
    for (id, tb) in bytes.iter().enumerate() {
        let id32 = id as u32;
        let expected = if id32 == end {
            m.is_complete()
        } else if special.contains(&id32) {
            false
        } else {
            let mut c = m.clone();
            c.accept_bytes(tb).is_ok()
        };
        assert_eq!(
            mask.is_allowed(id32),
            expected,
            "token {id} {:?} after {ctx:?}",
            String::from_utf8_lossy(tb)
        );
    }
}

fn check_equivalence(g: &Grammar, vocab: &Vocab, bytes: &[Vec<u8>], end: u32, special: &[u32], states: usize, seed: u64) {
    let mut rng = Rng(seed);
    for _ in 0..states {
        let call = gen_call(&mut rng);
        let b = call.as_bytes();
        let cut = rng.below(b.len() + 1);
        let mut m = Matcher::new(g);
        m.accept_bytes(&b[..cut]).unwrap_or_else(|_| panic!("a generated valid call was rejected: {call}"));
        assert_mask_matches(&m, vocab, bytes, end, special, &String::from_utf8_lossy(&b[..cut]));
    }
}

/// One random walk: sample uniformly among allowed tokens, biased toward closing after 40 bytes
/// inside a string or 60 steps. Returns the output when the end token became allowed.
fn walk(g: &Grammar, vocab: &Vocab, bytes: &[Vec<u8>], end: u32, rng: &mut Rng, max_steps: usize) -> Result<Option<Vec<u8>>, String> {
    let mut m = Matcher::new(g);
    let mut out = Vec::new();
    for step in 0..max_steps {
        let mut mask = TokenMask::new(bytes.len());
        m.allowed_tokens(vocab, &mut mask);
        if mask.is_allowed(end) {
            if !m.is_complete() {
                return Err("end token allowed before completion".into());
            }
            return Ok(Some(out));
        }
        let allowed: Vec<u32> = (0..bytes.len() as u32).filter(|&i| mask.is_allowed(i)).collect();
        if allowed.is_empty() {
            return Err(format!("dead end after {:?}", String::from_utf8_lossy(&out)));
        }
        let since_quote = out.iter().rev().take_while(|&&b| b != b'"').count();
        let mut pick = allowed[rng.below(allowed.len())];
        if (since_quote > 40 || step > 60) && rng.chance(60) {
            let closers: Vec<u32> = allowed
                .iter()
                .copied()
                .filter(|&i| bytes[i as usize].iter().any(|b| matches!(b, b'"' | b'}' | b']')))
                .collect();
            if !closers.is_empty() {
                pick = closers[rng.below(closers.len())];
            }
        }
        m.accept_token(vocab, pick).map_err(|_| format!("masked token {pick} was rejected by accept_token"))?;
        out.extend_from_slice(&bytes[pick as usize]);
    }
    Ok(None)
}

fn run_walks(vocab: &Vocab, bytes: &[Vec<u8>], end: u32, walks: usize, seed: u64, label: &str) {
    let g = grammar();
    let mut rng = Rng(seed);
    let mut completed = 0;
    for i in 0..walks {
        match walk(&g, vocab, bytes, end, &mut rng, 512) {
            Ok(Some(out)) => {
                validate(&out).unwrap_or_else(|e| panic!("{label} walk {i} produced an invalid call: {e}\n{}", String::from_utf8_lossy(&out)));
                completed += 1;
            }
            Ok(None) => {}
            Err(e) => panic!("{label} walk {i}: {e}"),
        }
    }
    eprintln!("grammar oracle: {label} walks completed {completed}/{walks}, dead ends 0");
    assert!(completed * 100 >= walks * 95, "{label}: only {completed}/{walks} walks completed within 512 tokens");
}

// ---------------------------------------------------------------------------------------------
// Tests.

const VALID: &[&str] = &[
    "{\"name\":\"read_file\",\"input\":{\"path\":\"/home/otter/notes.txt\"}}",
    "{\"name\": \"write_file\", \"input\": {\"path\": \"a.txt\", \"content\": \"line1\\nline2 \\\"q\\\" \\\\ \\u00e9 \\u65E5 \\/ \\t\\b\\f\\r\"}}",
    "{\"name\":\"run_command\",\"input\":{\"command\":\"echo résumé 日本語 🦦 \x7f\"}}",
    "{\"name\":\"open_app\",\"input\":{\"app\":\"editor\",\"path\":\"/docs/a.md\"}}",
    "{\"name\":\"open_app\",\"input\":{\"app\":\"terminal\"}}",
    "{\"name\":\"screenshot\",\"input\":{}}",
    "{\"name\":\"screenshot\",\"input\":{ }}",
    "{\"name\":\"list_dir\",\"input\":{\"path\":\"\"}}",
    "{ \"name\": \"fetch_url\", \"input\": { \"url\": \"https://example.com/?q=1&r=2\"}}",
    "{\"name\":\"synthetic\",\"input\":{\"n\":-42,\"flag\":true,\"nothing\":null,\"tags\":[\"x\", \"y\"],\"inner\":{\"k\":\"b\"}}}",
    "{\"name\":\"synthetic\",\"input\":{\"n\":0,\"tags\":[]}}",
    "{\"name\":\"synthetic\",\"input\":{\"n\":123456789012345678,\"flag\":false,\"tags\":[ \"\", \"a\", \"b\"]}}",
    "{\"name\":\"synthetic\",\"input\":{\"n\":-7,\"tags\":[ ]}}",
];

/// (accepted prefix, rest whose FIRST byte must be rejected, what it checks)
const REJECT: &[(&str, &str, &str)] = &[
    ("", " {", "no leading whitespace"),
    ("{\"", "input\":{}}", "the first key is name"),
    ("{\"name\":\"", "delete_file\",\"input\":{}}", "unknown tool name"),
    ("{\"name\":\"read_fil", "x\"", "tool name prefix"),
    ("{\"name\":\"read_file", "s\"", "no longer tool name"),
    ("{\"name\":\"read_file\"", "}", "input is required"),
    ("{\"name\":\"read_file\",\"input\":{\"pa", "ht\":\"x\"}}", "unknown key"),
    ("{\"name\":\"open_app\",\"input\":{\"", "path\":\"a\",\"app\":\"files\"}}", "keys in schema order"),
    ("{\"name\":\"write_file\",\"input\":{\"path\":\"a\"", "}}", "missing required key"),
    ("{\"name\":\"write_file\",\"input\":{\"path\":\"a\",", "}}", "trailing comma"),
    ("{\"name\":\"read_file\",\"input\":{\"path\":\"a\"", ",\"x\":1}}", "no key can follow"),
    ("{\"name\":\"read_file\",\"input\":{\"path\":", "null}}", "string expected"),
    ("{\"name\":\"open_app\",\"input\":{\"app\":\"", "browser\"}}", "value not in the enum"),
    ("{\"name\":\"open_app\",\"input\":{\"app\":\"files", "ystem\"}}", "no longer enum value"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"a", "\nb\"}}", "raw newline in a string"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"a", "\u{1}b\"}}", "raw control character"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"\\uD", "800\"}}", "surrogate escape"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"\\ud", "bff\"}}", "surrogate escape, lower case"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"\\", "x\"}}", "invalid escape"),
    ("{\"name\":\"run_command\",\"input\":{\"command\":\"\\u00", "g0\"}}", "non-hex digit"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":0", "1,\"tags\":[]}}", "leading zero"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":-0", "5,\"tags\":[]}}", "leading zero after a minus"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":-", "a,\"tags\":[]}}", "digit after a minus"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":123456789012345678", "9,\"tags\":[]}}", "at most 18 digits"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1", ".5,\"tags\":[]}}", "integers only"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":", "\"5\",\"tags\":[]}}", "integer expected"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"flag\":", "1,\"tags\":[]}}", "boolean expected"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"flag\":tru", "x,\"tags\":[]}}", "literal true"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"tags\":[\"a\",\"b\",\"c\"", ",\"d\"]}}", "maxItems"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"tags\":[", "1]}}", "array items are strings"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"tags\":[],\"inner\":{\"k\":\"", "c\"}}}", "nested enum"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"tags\":[],\"inner\":{", "}}}", "nested required key"),
    ("{\"name\":\"synthetic\",\"input\":{\"n\":1,\"tags\":[],\"", "flag\":true}}", "optional key out of order"),
    ("{\"name\":\"screenshot\",\"input\":{}}", "x", "text after the value"),
    ("{\"name\":\"screenshot\",\"input\":{}}", " ", "whitespace after the value"),
    ("{\"name\": ", " \"screenshot\"", "at most one space"),
    ("{\"name\"", " :", "no space before a colon"),
    ("{\"name\":", "\t\"screenshot\"", "spaces only, no tabs"),
    ("{\"name\":\"list_dir\",\"input\":{\"path\":\"a\"", " }}", "no space before a closing brace"),
    ("{\"name\":\"list_dir\",\"input\":{\"path\":\"a\"}", ",", "no extra top-level keys"),
];

#[test]
fn compiles_the_tool_set_and_rejects_unsupported_keywords() {
    grammar();
    let bad = [("bad", r#"{"type":"object","properties":{"s":{"type":"string","pattern":"^a"}}}"#)];
    let err = Grammar::tool_call(&bad, b"", b"").err().expect("pattern is unsupported");
    assert!(format!("{err}").contains("pattern"), "the error names the keyword: {err}");
    assert!(Grammar::tool_call(&[("bad", r#"{"type":"string"}"#)], b"", b"").is_err(), "tool input must be an object");
}

#[test]
fn accepts_valid_calls_and_completes_only_at_the_end() {
    let g = grammar();
    for call in VALID {
        validate(call.as_bytes()).unwrap_or_else(|e| panic!("test data invalid ({e}): {call}"));
        let mut m = Matcher::new(&g);
        for (i, byte) in call.as_bytes().iter().enumerate() {
            assert!(!m.is_complete(), "complete before byte {i} of {call}");
            m.accept_bytes(std::slice::from_ref(byte)).unwrap_or_else(|_| panic!("byte {i} of valid call {call}"));
        }
        assert!(m.is_complete(), "valid call not complete: {call}");
        assert!(accepts_complete(&g, call.as_bytes()), "whole-slice accept of {call}");
    }
}

#[test]
fn rejects_at_the_offending_byte() {
    let g = grammar();
    for (prefix, rest, why) in REJECT {
        let s = [*prefix, *rest].concat();
        assert_eq!(first_rejection(&g, s.as_bytes()), Some(prefix.len()), "{why}: {s:?}");
    }
}

#[test]
fn max_length_counts_characters_after_unescaping() {
    let g = grammar();
    let base = "{\"name\":\"read_file\",\"input\":{\"path\":\"";
    let exact = [base, &"a".repeat(200), "\\n", &"é".repeat(55), "\"}}"].concat();
    assert!(accepts_complete(&g, exact.as_bytes()), "exactly 256 characters is allowed");
    for extra in ["a", "é", "\\n", "\\u00e9", "🦦"] {
        let full = [base, &"a".repeat(256)].concat();
        let s = [full.as_str(), extra, "\"}}"].concat();
        assert_eq!(first_rejection(&g, s.as_bytes()), Some(full.len()), "257th character {extra:?}");
    }
}

#[test]
fn rejects_invalid_utf8_and_accepts_split_characters() {
    let g = grammar();
    let base = b"{\"name\":\"run_command\",\"input\":{\"command\":\"".to_vec();
    let cases: &[(&[u8], usize)] = &[
        (&[0xFF], 0),
        (&[0xF5, 0x80], 0),
        (&[0xC0, 0x80], 0),
        (&[0xC1, 0xBF], 0),
        (&[0x80], 0),
        (&[0xE0, 0x80, 0x80], 1),
        (&[0xED, 0xA0, 0x80], 1),
        (&[0xF0, 0x80, 0x80, 0x80], 1),
        (&[0xF4, 0x90, 0x80, 0x80], 1),
        (&[0xE6, 0x97, b'"'], 2),
        (&[0xC3, b'a'], 1),
    ];
    for (bad, at) in cases {
        let mut s = base.clone();
        s.extend_from_slice(bad);
        assert_eq!(first_rejection(&g, &s), Some(base.len() + at), "bytes {bad:02x?}");
    }
    let mut m = Matcher::new(&g);
    m.accept_bytes(&base).unwrap();
    for part in [&[0xE6u8][..], &[0x97], &[0xA5], &[0xF0, 0x9F], &[0xA6, 0xA6], b"\"}}"] {
        m.accept_bytes(part).expect("a character split across tokens");
    }
    assert!(m.is_complete());
}

#[test]
fn prefix_and_suffix_literals() {
    let g = Grammar::tool_call(TOOLS, b"<tool_call>", b"</tool_call>").expect("compiles");
    let full = ["<tool_call>", "{\"name\":\"screenshot\",\"input\":{}}", "</tool_call>"].concat();
    assert!(accepts_complete(&g, full.as_bytes()));
    let mut m = Matcher::new(&g);
    m.accept_bytes(&full.as_bytes()[..full.len() - 1]).unwrap();
    assert!(!m.is_complete(), "not complete before the suffix ends");
    assert_eq!(first_rejection(&g, b"<tool_calx"), Some(9));
    assert_eq!(first_rejection(&g, b"{"), Some(0), "the prefix is required");
    let after = [full.as_str(), " "].concat();
    assert_eq!(first_rejection(&g, after.as_bytes()), Some(full.len()));
    m.reset();
    assert!(m.accept_bytes(full.as_bytes()).is_ok() && m.is_complete(), "reset starts over");
}

#[test]
fn generated_calls_are_valid_and_accepted() {
    let g = grammar();
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..3000 {
        let call = gen_call(&mut rng);
        validate(call.as_bytes()).unwrap_or_else(|e| panic!("generator produced an invalid call ({e}): {call}"));
        assert!(accepts_complete(&g, call.as_bytes()), "rejected: {call}");
    }
}

#[test]
fn mask_equals_brute_force_synthetic_vocab() {
    let (vocab, bytes, end, special) = synthetic(&mut Rng(0x5eed_0002));
    check_equivalence(&grammar(), &vocab, &bytes, end, &special, 300, 0x5eed_0003);
    let g = Grammar::tool_call(TOOLS, b"<tool_call>", b"</tool_call>").expect("compiles");
    let mut m = Matcher::new(&g);
    assert_mask_matches(&m, &vocab, &bytes, end, &special, "start of the prefix");
    m.accept_bytes(b"<tool_call>{\"name\":\"screenshot\",\"input\":{}}</tool").unwrap();
    assert_mask_matches(&m, &vocab, &bytes, end, &special, "inside the suffix");
}

#[test]
fn mask_equals_brute_force_real_vocab() {
    let Some(r) = real() else { return };
    check_equivalence(&grammar(), &r.vocab, &r.bytes, r.end, &r.special, 100, 0x5eed_0004);
}

#[test]
fn random_walks_synthetic_vocab() {
    let (vocab, bytes, end, _) = synthetic(&mut Rng(0x5eed_0005));
    run_walks(&vocab, &bytes, end, 2000, 0x5eed_0006, "synthetic");
}

#[test]
fn random_walks_real_vocab() {
    let Some(r) = real() else { return };
    run_walks(&r.vocab, &r.bytes, r.end, 1000, 0x5eed_0007, "SmolLM2");
}

#[test]
fn smollm2_tokenizations_of_valid_calls_are_accepted() {
    let Some(r) = real() else { return };
    let g = grammar();
    let mut rng = Rng(0x5eed_0008);
    for i in 0..1000 {
        let call = gen_call(&mut rng);
        let ids = r.tokenizer.encode(&call).expect("encode");
        assert_eq!(r.tokenizer.decode(&ids).expect("decode"), call.as_bytes(), "tokenizer round trip");
        let mut m = Matcher::new(&g);
        for (k, &id) in ids.iter().enumerate() {
            assert!(!m.is_complete(), "call {i} complete before token {k}: {call}");
            m.accept_token(&r.vocab, id).unwrap_or_else(|_| {
                panic!("call {i}: token {k} {:?} rejected in {call}", String::from_utf8_lossy(&r.bytes[id as usize]))
            });
        }
        assert!(m.is_complete(), "call {i} not complete: {call}");
        if i % 50 == 0 {
            let mut mask = TokenMask::new(r.bytes.len());
            m.allowed_tokens(&r.vocab, &mut mask);
            assert!(mask.is_allowed(r.end) && mask.count_allowed() == 1, "only the end token after a complete call");
        }
    }
}

#[test]
fn mask_speed_real_vocab() {
    let Some(r) = real() else { return };
    let g = grammar();
    let mut rng = Rng(0x5eed_0009);
    // Long free strings are the expensive states: most of the vocabulary is allowed there.
    let long = ["{\"name\":\"write_file\",\"input\":{\"path\":\"notes.txt\",\"content\":\"", &"Otters hold hands while they sleep. ".repeat(20)].concat();
    let mut calls = vec![long];
    calls.extend((0..20).map(|_| gen_call(&mut rng)));
    let (mut total, mut max, mut n) = (0.0f64, 0.0f64, 0usize);
    for call in &calls {
        let mut m = Matcher::new(&g);
        for id in r.tokenizer.encode(call).expect("encode") {
            let mut mask = TokenMask::new(r.bytes.len());
            let t = Instant::now();
            m.allowed_tokens(&r.vocab, &mut mask);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            total += ms;
            max = max.max(ms);
            n += 1;
            assert!(mask.is_allowed(id), "the next real token is allowed");
            m.accept_token(&r.vocab, id).unwrap();
        }
    }
    let mean = total / n as f64;
    eprintln!("grammar oracle: mask time over {n} SmolLM2 steps: mean {mean:.3} ms, max {max:.3} ms");
    assert!(mean < 20.0, "mean mask time {mean:.3} ms");
}
