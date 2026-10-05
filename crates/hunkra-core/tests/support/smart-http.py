"""Finite-test subprocess serving only a temporary Git root on loopback; its Rust owner reaps it."""
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit
import os
import subprocess
import ssl
import sys


class Handler(BaseHTTPRequestHandler):
    """Use independent Git CGI as the fixture's smart protocol oracle, with basic auth."""

    def log_message(self, *_):
        """Suppress arbitrary request/header diagnostics in fixture output."""

    def do_GET(self):
        """Serve discovery only after fixture credentials are accepted."""
        self.git_request()

    def do_POST(self):
        """Serve upload-pack through the same authenticated independent backend."""
        self.git_request()

    def git_request(self):
        """Translate a bounded request into CGI variables without reading any owner configuration."""
        if self.headers.get("Authorization") != "Basic Zml4dHVyZTpzZWNyZXQ=":
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="fixture"')
            self.end_headers()
            return
        url = urlsplit(self.path)
        size = int(self.headers.get("Content-Length", "0"))
        if size > 1024 * 1024:
            self.send_error(413)
            return
        body = self.rfile.read(size)
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            parts = []
            total = 0
            while True:
                size = int(self.rfile.readline().split(b";", 1)[0], 16)
                if size == 0:
                    self.rfile.readline()
                    break
                total += size
                if total > 1024 * 1024:
                    self.send_error(413)
                    return
                parts.append(self.rfile.read(size))
                assert self.rfile.read(2) == b"\r\n"
            body = b"".join(parts)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(GIT_PROJECT_ROOT=sys.argv[1], GIT_HTTP_EXPORT_ALL="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", PATH_INFO=url.path, QUERY_STRING=url.query, REQUEST_METHOD=self.command, CONTENT_TYPE=self.headers.get("Content-Type", ""), CONTENT_LENGTH=str(len(body)), REMOTE_USER="fixture", HTTP_GIT_PROTOCOL=self.headers.get("Git-Protocol", ""))
        result = subprocess.run(["/usr/bin/git", "http-backend"], input=body, env=environment, capture_output=True, timeout=15)
        headers, separator, body = result.stdout.partition(b"\r\n\r\n")
        if result.returncode != 0 or not separator:
            self.send_error(500)
            return
        fields = [line.split(b":", 1) for line in headers.split(b"\r\n") if b":" in line]
        status = next((int(value.strip().split()[0]) for key, value in fields if key.lower() == b"status"), 200)
        self.send_response(status)
        for key, value in fields:
            if key.lower() != b"status":
                self.send_header(key.decode(), value.decode().strip())
        self.end_headers()
        self.wfile.write(body)


server = HTTPServer(("127.0.0.1", 0), Handler)
if len(sys.argv) == 4:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(sys.argv[2], sys.argv[3])
    server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_port, flush=True)
server.serve_forever()
