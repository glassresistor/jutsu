#!/usr/bin/env python3
"""Serve the jutsu WASM demo. Usage: python wasm/serve.py [port]"""
import http.server
import mimetypes
import os
import sys

# Register .wasm MIME type (required by browsers for WebAssembly)
mimetypes.add_type('application/wasm', '.wasm')

port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
dir = os.path.dirname(os.path.abspath(__file__))

os.chdir(dir)
print(f'\n  jutsu WASM demo: http://localhost:{port}\n')
print('  Press Ctrl+C to stop.\n')

httpd = http.server.HTTPServer(('', port), http.server.SimpleHTTPRequestHandler)
httpd.serve_forever()
