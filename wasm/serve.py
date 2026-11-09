#!/usr/bin/env python3
"""Serve the jutsu WASM demo. Usage: python wasm/serve.py [port]"""
import http.server
import os
import sys


class WasmHandler(http.server.SimpleHTTPRequestHandler):
    """SimpleHTTPRequestHandler with correct .wasm MIME type."""

    def end_headers(self):
        # Ensure .wasm files get the right Content-Type
        path = self.path.split('?')[0]
        if path.endswith('.wasm'):
            self.send_header('Content-Type', 'application/wasm')
        super().end_headers()

    def guess_type(self, path):
        if path.endswith('.wasm'):
            return 'application/wasm'
        return super().guess_type(path)


port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
dir = os.path.dirname(os.path.abspath(__file__))

os.chdir(dir)
print(f'\n  jutsu WASM demo: http://localhost:{port}\n')
print('  Press Ctrl+C to stop.\n')

httpd = http.server.HTTPServer(('', port), WasmHandler)
httpd.serve_forever()
