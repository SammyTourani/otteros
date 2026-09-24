#!/usr/bin/env python3
"""
Claude Messages API mock server for testing.

Stdlib only. Listens on 127.0.0.1 on a free port (or specified via --port).
Validates request headers and body shape.
Replays a scripted multi-turn tool conversation over SSE.
Can log requests to a JSON-lines file via --log argument.
"""

import http.server
import json
import ssl
import sys
import time
from pathlib import Path
import argparse

PORT = 0  # 0 means find a free port
HOST = "127.0.0.1"
LOG_FILE = None

# SSE response for a simple text message
SIMPLE_TEXT_RESPONSE = """event: message_start
data: {"message": {"id": "msg_001", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello! "}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "I'm here to help."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 10}}

event: message_stop
data: {"type": "message_stop"}

"""

# SSE response for a tool call
TOOL_CALL_RESPONSE = """event: message_start
data: {"message": {"id": "msg_002", "model": "claude-opus-5"}, "type": "message_start"}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tool_123", "name": "calculate"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "\"x\": 10, "}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "\"y\": 5"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 20}}

event: message_stop
data: {"type": "message_stop"}

"""

class MockClaudeHandler(http.server.BaseHTTPRequestHandler):
    """Handler for mock Claude API requests."""

    def log_message(self, format, *args):
        """Suppress default logging."""
        pass

    def do_POST(self):
        """Handle POST request to /v1/messages."""
        if self.path != "/v1/messages":
            self.send_error(404, "Not found")
            return

        # Check required headers
        api_key = self.headers.get("x-api-key")
        if not api_key:
            self.send_error(400, "Missing x-api-key header")
            return

        # Check anthropic-version header
        version = self.headers.get("anthropic-version")
        if version != "2023-06-01":
            self.send_error(400, "Invalid anthropic-version")
            return

        # Check beta header
        beta = self.headers.get("anthropic-beta")
        if "server-side-fallback-2026-07-01" not in (beta or ""):
            self.send_error(400, "Invalid anthropic-beta header")
            return

        # Read request body
        content_length = int(self.headers.get("content-length", 0))
        body = self.rfile.read(content_length)

        try:
            request_data = json.loads(body)
        except json.JSONDecodeError:
            self.send_error(400, "Invalid JSON in request body")
            return

        # Validate request body structure
        required_fields = ["model", "max_tokens", "stream", "messages"]
        for field in required_fields:
            if field not in request_data:
                self.send_error(400, f"Missing required field: {field}")
                return

        if request_data.get("stream") is not True:
            self.send_error(400, "stream must be true")
            return

        # Log request if logging is enabled
        if LOG_FILE:
            self._log_request(request_data)

        # Determine response based on message content
        response = self._get_response(request_data)

        # Send SSE response
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "keep-alive")
        self.end_headers()

        # Send SSE data
        self.wfile.write(response.encode("utf-8"))
        self.wfile.flush()

    def _log_request(self, request_data):
        """Log request to JSON-lines file."""
        try:
            log_entry = {
                "headers": dict(self.headers),
                "body": request_data,
            }
            with open(LOG_FILE, "a") as f:
                f.write(json.dumps(log_entry) + "\n")
        except Exception:
            pass  # Ignore logging errors

    def _get_response(self, request_data):
        """Get response based on request content."""
        messages = request_data.get("messages", [])

        # If the last message is a user message with tool_result content, send a text response
        if messages and messages[-1].get("role") == "user":
            last_content = messages[-1].get("content", [])
            # Check if there's a tool_result in the content
            has_tool_result = any(
                isinstance(c, dict) and c.get("type") == "tool_result"
                for c in last_content
            )
            if has_tool_result:
                # Send a simple text response as a continuation
                return SIMPLE_TEXT_RESPONSE

        # Default: send a tool call response to start the interaction
        return TOOL_CALL_RESPONSE


def main():
    """Start the mock Claude server."""
    global PORT, LOG_FILE

    # Parse arguments
    parser = argparse.ArgumentParser(description="Claude Messages API mock server")
    parser.add_argument("--port", type=int, default=0, help="Port to listen on (0 for auto)")
    parser.add_argument("--log", type=str, default=None, help="Log file for requests (JSON-lines format)")
    args = parser.parse_args()

    PORT = args.port
    LOG_FILE = args.log

    # Create server
    server_address = (HOST, PORT)
    httpd = http.server.HTTPServer(server_address, MockClaudeHandler)

    # Get the actual port if 0 was specified
    actual_port = httpd.server_address[1]
    PORT = actual_port

    # Print port to stdout for test reading
    print(f"{actual_port}")
    sys.stdout.flush()

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        httpd.shutdown()


if __name__ == "__main__":
    main()
