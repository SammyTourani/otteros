#!/usr/bin/env python3
"""
HTTP test server for otter-http integration tests.

Serves various endpoints for testing:
- /fixed: Fixed-length response with Content-Length header
- /chunked: Transfer-Encoding: chunked response
- /redirect: HTTP redirects (301, 302, 303, 307, 308)
- /sse: Server-sent events stream
"""

import sys
from http.server import HTTPServer, BaseHTTPRequestHandler
from io import BytesIO
import time

class TestHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/fixed':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            body = b'Hello, World!'
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        elif self.path == '/chunked':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.send_header('Transfer-Encoding', 'chunked')
            self.end_headers()

            # Send chunks
            chunks = [b'Hello, ', b'World', b'!']
            for chunk in chunks:
                self.wfile.write(f'{len(chunk):x}\r\n'.encode())
                self.wfile.write(chunk)
                self.wfile.write(b'\r\n')

            # Final chunk
            self.wfile.write(b'0\r\n\r\n')

        elif self.path == '/redirect-301':
            self.send_response(301)
            self.send_header('Location', '/redirected')
            self.end_headers()

        elif self.path == '/redirect-302':
            self.send_response(302)
            self.send_header('Location', '/redirected')
            self.end_headers()

        elif self.path == '/redirect-303':
            self.send_response(303)
            self.send_header('Location', '/redirected')
            self.end_headers()

        elif self.path == '/redirect-307':
            self.send_response(307)
            self.send_header('Location', '/redirected')
            self.end_headers()

        elif self.path == '/redirect-308':
            self.send_response(308)
            self.send_header('Location', '/redirected')
            self.end_headers()

        elif self.path == '/redirected':
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            body = b'Redirected'
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        elif self.path == '/sse':
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Cache-Control', 'no-cache')
            self.end_headers()

            # Send some test events
            events = [
                b'event: message\r\ndata: Hello\r\n\r\n',
                b'data: Line 1\r\ndata: Line 2\r\nid: 123\r\n\r\n',
                b'retry: 5000\r\ndata: Please retry\r\n\r\n',
            ]

            for event in events:
                self.wfile.write(event)
                self.wfile.flush()

        elif self.path == '/204':
            self.send_response(204)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()

        elif self.path == '/304':
            self.send_response(304)
            self.send_header('Content-Type', 'text/plain')
            self.end_headers()

        else:
            self.send_response(404)
            self.send_header('Content-Type', 'text/plain')
            body = b'Not Found'
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def log_message(self, format, *args):
        # Suppress logging
        pass

if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8888
    server = HTTPServer(('127.0.0.1', port), TestHandler)
    print(f'Server running on port {port}', file=sys.stderr, flush=True)
    server.serve_forever()
