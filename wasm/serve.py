#!/usr/bin/env python3
"""Serve the jutsu WASM demo. Usage: python wasm/serve.py [port]"""
import http.server
import os
import sys

port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
dir = os.path.dirname(os.path.abspath(__file__))

os.chdir(dir)
print(f'\n  jutsu WASM demo: http://localhost:{port}\n')
print('  Press Ctrl+C to stop.\n')

handler = http.server.SimpleHTTPRequestHandler
httpd = http.server.HTTPServer(('', port), handler)
httpd.serve_forever()
