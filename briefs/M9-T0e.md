# Brief M9-T0e — otter-agent: Claude operates OtterOS through its tools, over our TLS and HTTP

## Goal
The headline of agent mode, proven on the host before the OS has TCP: a pure crate
`crates/otter-agent` that sends Messages API requests over `otter-tls` (TLS 1.3) and HTTP/1.1,
streams the SSE response into `otter-claude`'s tool loop, and executes tool calls through an
`otter-tools::Session` (validation, D30/D31 policy, Allow/Deny, action log, key redaction). The
kernel later supplies a TCP `Connector` and the desktop supplies the `Os`. Acceptance:
`crates/otter-agent/tests/oracle.rs` (orchestrator, unchanged; its header is the API and
semantics), which drives real TLS connections to `scripts/mock_claude.py` (orchestrator; D23).

## Applies
D2, D22, D23, D27 (no_std + alloc, `#![forbid(unsafe_code)]`, path dependencies only: otter-claude,
otter-tools, otter-tls, otter-x509, otter-http, otter-json), D30, D31. Files under ~600 lines
(e.g. `lib.rs`, `transport.rs` (TLS + HTTP over a ByteStream), `host.rs` (the ToolHost adapter),
`agent.rs`). Never run git checkout/restore/reset/stash/clean. Tests only via
`scripts/verify-crate.sh otter-agent` or `scripts/memguard.py`. Do not edit the oracle or the mock.

## Design
1. Transport (implements `otter_claude::Transport`): per request, `Connector::connect(host, port)`,
   `otter_tls::ClientConnection::new(ClientConfig::new(TrustStore::from_der_roots(roots)),
   server_name, now_unix, rng)`, pump `take_outgoing`/`write_all` and `read`/`read_tls`/`process`
   until `HandshakeComplete` (any TlsError or alert → `AgentError::Tls`, before anything is sent);
   send the HTTP/1.1 POST (headers from the oracle header, body from otter-claude's request); read
   the status line and headers, then the body (chunked, content-length or until close), handing
   the SSE bytes to otter-claude. A non-200 status: parse `{"type":"error","error":{"message":...}}`
   into `Api(message)`, else `Http(status, body)`. EOF or `PeerClosed` before `message_stop` is
   `Protocol`. Close with `close()` and flush.
2. ToolHost adapter: every tool reports read-only to the loop (the Session owns approval), and
   `execute(tool, input)` = `Session::call(tool, input)`: `Ok(text)` for a result, `Err(text)` when
   `is_error` (so the tool_result carries `is_error: true`). A `Png` content becomes the text
   `[screenshot: N-byte PNG]` for now (image tool results come with the desktop integration).
3. Agent: keeps the `otter_claude::Conversation` (system prompt, the seven tools from
   `otter_tools::TOOLS` with their JSON schemas parsed by otter-json, the history) across `ask`
   calls; `ask` appends the user text and runs the tool loop (at most 16 turns, else
   `TooManyTurns`), returning the final assistant text.
4. The API URL in otter-claude is fixed to api.anthropic.com; the Transport connects to
   `Backend.host:port` and uses `Backend.path` regardless, so the same code reaches the real API
   (host api.anthropic.com, port 443, server_name api.anthropic.com, the embedded Mozilla roots)
   and the mock.

## Acceptance
- `scripts/verify-crate.sh otter-agent` → exit 0; the oracle shows 9 passed.
- The oracle file and scripts/mock_claude.py are byte-for-byte unchanged.

## Report
<=15 lines: files, test counts, deviations, anything in the oracle or the mock you believe is wrong
(quote it). Do not commit.
