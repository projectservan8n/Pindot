#!/usr/bin/env python3
"""Serve a folder the way Tauri serves Pindot's pages: with the app's
Content-Security-Policy plus a nonce on inline <style> and <script> tags.
Tauri adds that nonce at build time, and once a nonce is in style-src the
browser ignores 'unsafe-inline': every style="..." attribute in the markup
is dropped. A plain file server (and headless Chrome on it) hides that.

usage: csp_serve.py <folder> <port> [path/to/tauri.conf.json]
"""
import http.server, json, re, sys, os, functools

folder, port = sys.argv[1], int(sys.argv[2])
conf = sys.argv[3] if len(sys.argv) > 3 else os.path.expanduser('~/deskhand/app/src-tauri/tauri.conf.json')
csp = json.load(open(conf))['app']['security']['csp']
NONCE = 'pindottest'
def with_nonce(policy):
    out = []
    for part in [p.strip() for p in policy.split(';') if p.strip()]:
        if part.startswith('script-src') or part.startswith('style-src'):
            part += " 'nonce-%s'" % NONCE
        out.append(part)
    return '; '.join(out)
CSP = with_nonce(csp)

class H(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        path = self.translate_path(self.path.split('?')[0].split('#')[0])
        if os.path.isdir(path): path = os.path.join(path, 'index.html')
        if path.endswith('.html') and os.path.exists(path):
            body = open(path, encoding='utf-8').read()
            body = re.sub(r'<(style|script)(?![^>]*\bsrc=)([^>]*)>', r'<\1 nonce="%s"\2>' % NONCE, body)
            body = re.sub(r'<script([^>]*\bsrc=[^>]*)>', r'<script nonce="%s"\1>' % NONCE, body)
            data = body.encode('utf-8')
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Security-Policy', CSP)
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        super().do_GET()

print('CSP:', CSP, flush=True)
http.server.ThreadingHTTPServer(('127.0.0.1', port), functools.partial(H, directory=folder)).serve_forever()
