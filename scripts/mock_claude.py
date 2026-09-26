#!/usr/bin/env python3
"""A scripted stand-in for the Claude Messages API (D23: automated tests never call the real
API), written by the orchestrator for briefs M9-T0e (host end-to-end) and M9's QEMU test.

HTTPS with TLS 1.3 only, using the test PKI (crates/otter-tls/tests/fixtures/server: leaf for
leaf.otter-test.example, chained to the intermediate and the test root). One request per
connection (the response carries `connection: close`). Every request is validated against the
scenario; a mismatch is answered with HTTP 400 and an `invalid_request_error` whose message starts
with "mock:" so the client surfaces it. Replies stream as SSE over chunked transfer encoding in
1-17 byte chunks.

Scenario JSON:
  {"api_key": "...", "tools": [names in order],
   "turns": [
     {"expect": {"last_user_text": "...",            # the last message is a user text message
                 "tool_result": {"tool_use_id": "...", "content": "..." | "content_prefix": "...",
                                 "is_error": false},
                 "message_count": 3,                  # exact number of messages in the request
                 "echo_signature": "sig-1"},          # an earlier assistant thinking block, echoed
      "reply": [{"thinking": "...", "signature": "..."}, {"text": "..."},
                {"tool_use": {"id": "...", "name": "...", "input": {...}}}],
      "stop_reason": "tool_use" | "end_turn",
      "http_error": {"status": 529, "type": "overloaded_error", "message": "Overloaded"},
      "close_after_bytes": 200}                       # drop the connection mid-stream
   ]}

Usage: mock_claude.py --scenario FILE --log FILE [--port 0] [--host 127.0.0.1]
Prints `LISTENING <port>` once ready. Each request is appended to the log as one JSON line:
{"turn": n, "path", "headers": {lowercased name: value, x-api-key replaced by "match"/"mismatch"},
 "body": parsed JSON or null, "error": null or the mismatch}.
"""
import argparse
import json
import os
import random
import socket
import ssl
import sys
import tempfile
import threading

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CERTS = os.path.join(ROOT, "crates", "otter-tls", "tests", "fixtures", "server")


def tls_context():
    chain = tempfile.NamedTemporaryFile("w", suffix=".pem", delete=False)
    with open(os.path.join(CERTS, "leaf.pem")) as leaf, open(os.path.join(CERTS, "intermediate.pem")) as inter:
        chain.write(leaf.read() + inter.read())
    chain.close()
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_3
    ctx.load_cert_chain(chain.name, os.path.join(CERTS, "leaf.key"))
    os.unlink(chain.name)
    return ctx


def read_request(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        piece = conn.recv(65536)
        if not piece:
            return None
        data += piece
        if len(data) > 1 << 20:
            return None
    head, rest = data.split(b"\r\n\r\n", 1)
    lines = head.decode("latin-1").split("\r\n")
    method, path, _ = (lines[0].split(" ") + ["", "", ""])[:3]
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    length = int(headers.get("content-length", "0") or 0)
    while len(rest) < length:
        piece = conn.recv(65536)
        if not piece:
            break
        rest += piece
    return method, path, headers, rest[:length]


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text")
    return None


def check(turn, scenario, method, path, headers, body):
    if method != "POST" or path != "/v1/messages":
        return f"mock: expected POST /v1/messages, got {method} {path}"
    if headers.get("x-api-key") != scenario["api_key"]:
        return "mock: x-api-key does not match"
    if headers.get("anthropic-version") != "2023-06-01":
        return f"mock: anthropic-version {headers.get('anthropic-version')!r}"
    if not headers.get("content-type", "").startswith("application/json"):
        return "mock: content-type must be application/json"
    if body is None:
        return "mock: body is not JSON"
    if body.get("model") != "claude-opus-5" or body.get("stream") is not True or not body.get("max_tokens"):
        return "mock: model claude-opus-5, stream true and max_tokens are required"
    names = [t.get("name") for t in body.get("tools", [])]
    if names != scenario["tools"]:
        return f"mock: tools {names} != {scenario['tools']}"
    messages = body.get("messages") or []
    if not messages:
        return "mock: no messages"
    expect = turn.get("expect", {})
    last = messages[-1]
    if "last_user_text" in expect:
        if last.get("role") != "user" or text_of(last.get("content")) != expect["last_user_text"]:
            return f"mock: last user text {text_of(last.get('content'))!r} != {expect['last_user_text']!r}"
    if "tool_result" in expect:
        want = expect["tool_result"]
        blocks = last.get("content") if last.get("role") == "user" and isinstance(last.get("content"), list) else []
        results = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_result"]
        match = [b for b in results if b.get("tool_use_id") == want["tool_use_id"]]
        if len(match) != 1:
            return f"mock: expected one tool_result for {want['tool_use_id']}, got {results}"
        got = match[0]
        text = text_of(got.get("content")) or ""
        if "content" in want and text != want["content"]:
            return f"mock: tool_result {text!r} != {want['content']!r}"
        if "content_prefix" in want and not text.startswith(want["content_prefix"]):
            return f"mock: tool_result {text!r} does not start with {want['content_prefix']!r}"
        if bool(got.get("is_error", False)) != want.get("is_error", False):
            return f"mock: tool_result is_error {got.get('is_error', False)} != {want.get('is_error', False)}"
    if "message_count" in expect and len(messages) != expect["message_count"]:
        return f"mock: {len(messages)} messages, expected {expect['message_count']}"
    if "echo_signature" in expect:
        sig = expect["echo_signature"]
        echoed = any(
            m.get("role") == "assistant"
            and isinstance(m.get("content"), list)
            and any(isinstance(b, dict) and b.get("type") == "thinking" and b.get("signature") == sig for b in m["content"])
            for m in messages
        )
        if not echoed:
            return f"mock: the thinking block with signature {sig!r} was not echoed back"
    return None


def sse(turn, message_id):
    out = []

    def event(name, data):
        out.append(f"event: {name}\ndata: {json.dumps(data, separators=(',', ':'))}\n\n")

    event("message_start", {"type": "message_start", "message": {"id": message_id, "type": "message", "role": "assistant",
                                                              "model": "claude-opus-5", "content": [], "stop_reason": None,
                                                              "usage": {"input_tokens": 50, "output_tokens": 1}}})
    for index, block in enumerate(turn.get("reply", [])):
        if "thinking" in block:
            start = {"type": "thinking", "thinking": "", "signature": ""}
            deltas = [{"type": "thinking_delta", "thinking": block["thinking"]},
                      {"type": "signature_delta", "signature": block["signature"]}]
        elif "text" in block:
            t = block["text"]
            start = {"type": "text", "text": ""}
            deltas = [{"type": "text_delta", "text": t[: len(t) // 2]}, {"type": "text_delta", "text": t[len(t) // 2:]}]
        else:
            use = block["tool_use"]
            start = {"type": "tool_use", "id": use["id"], "name": use["name"], "input": {}}
            raw = json.dumps(use["input"], separators=(",", ":"))
            cuts = sorted({0, len(raw) // 3, 2 * len(raw) // 3, len(raw)})
            deltas = [{"type": "input_json_delta", "partial_json": raw[a:b]} for a, b in zip(cuts, cuts[1:]) if b > a]
        event("content_block_start", {"type": "content_block_start", "index": index, "content_block": start})
        event("ping", {"type": "ping"})
        for d in deltas:
            event("content_block_delta", {"type": "content_block_delta", "index": index, "delta": d})
        event("content_block_stop", {"type": "content_block_stop", "index": index})
    event("message_delta", {"type": "message_delta", "delta": {"stop_reason": turn.get("stop_reason", "end_turn"), "stop_sequence": None},
                            "usage": {"output_tokens": 40}})
    event("message_stop", {"type": "message_stop"})
    return "".join(out).encode()


def send_chunked(conn, payload, rng, close_after=None):
    sent = 0
    i = 0
    while i < len(payload):
        n = rng.randint(1, 17)
        piece = payload[i:i + n]
        frame = f"{len(piece):x}\r\n".encode() + piece + b"\r\n"
        if close_after is not None and sent + len(frame) > close_after:
            conn.sendall(frame[: max(0, close_after - sent)])
            return False
        conn.sendall(frame)
        sent += len(frame)
        i += n
    conn.sendall(b"0\r\n\r\n")
    return True


def serve(listener, ctx, scenario, log_path):
    rng = random.Random(1234)
    turn_no = 0
    while True:
        raw, _ = listener.accept()
        try:
            conn = ctx.wrap_socket(raw, server_side=True)
        except (ssl.SSLError, OSError):
            raw.close()
            continue
        try:
            req = read_request(conn)
            if req is None:
                continue
            method, path, headers, body_bytes = req
            try:
                body = json.loads(body_bytes)
            except ValueError:
                body = None
            turns = scenario["turns"]
            turn = turns[turn_no] if turn_no < len(turns) else None
            error = "mock: no more turns in the scenario" if turn is None else check(turn, scenario, method, path, headers, body)
            logged = dict(headers)
            if "x-api-key" in logged:
                logged["x-api-key"] = "match" if logged["x-api-key"] == scenario["api_key"] else "mismatch"
            with open(log_path, "a") as log:
                log.write(json.dumps({"turn": turn_no, "path": path, "headers": logged, "body": body, "error": error}) + "\n")
            turn_no += 1
            if error or (turn and "http_error" in turn):
                if error:
                    status, etype, msg = 400, "invalid_request_error", error
                else:
                    e = turn["http_error"]
                    status, etype, msg = e["status"], e["type"], e["message"]
                payload = json.dumps({"type": "error", "error": {"type": etype, "message": msg}}).encode()
                conn.sendall(f"HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {len(payload)}\r\nconnection: close\r\n\r\n".encode() + payload)
                continue
            conn.sendall(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n")
            send_chunked(conn, sse(turn, f"msg_{turn_no:02d}"), rng, turn.get("close_after_bytes"))
        except (ssl.SSLError, OSError):
            pass
        finally:
            try:
                conn.close()
            except Exception:
                pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenario", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--host", default="127.0.0.1")
    args = ap.parse_args()
    scenario = json.load(open(args.scenario))
    open(args.log, "w").close()
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind((args.host, args.port))
    listener.listen(8)
    print(f"LISTENING {listener.getsockname()[1]}", flush=True)
    t = threading.Thread(target=serve, args=(listener, tls_context(), scenario, args.log), daemon=True)
    t.start()
    try:
        sys.stdin.read()  # runs until the parent closes stdin or kills us
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
