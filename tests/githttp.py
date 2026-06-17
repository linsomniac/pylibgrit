"""A minimal smart-HTTP server backed by `git http-backend`, for hermetic https tests.

AIDEV-NOTE: Python 3.13 removed CGIHTTPRequestHandler, so we run `git http-backend` ourselves as a
subprocess per request, translating HTTP <-> CGI. Supports optional HTTP Basic auth. Threaded so the
smart-HTTP multi-request dance does not deadlock. Listens on 127.0.0.1; not for production use.
"""

from __future__ import annotations

import base64
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def make_handler(project_root: Path, env: dict[str, str], auth: tuple[str, str] | None):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def _denied(self) -> None:
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="git"')
            self.send_header("Content-Length", "0")
            self.end_headers()

        def _check_auth(self) -> bool:
            if auth is None:
                return True
            header = self.headers.get("Authorization", "")
            if not header.startswith("Basic "):
                return False
            try:
                user, _, pw = base64.b64decode(header[6:]).decode().partition(":")
            except Exception:
                return False
            return (user, pw) == auth

        def _run_backend(self, body: bytes) -> None:
            if not self._check_auth():
                self._denied()
                return
            path, _, query = self.path.partition("?")
            cgi_env = dict(env)
            cgi_env.update(
                {
                    "GIT_PROJECT_ROOT": str(project_root),
                    "GIT_HTTP_EXPORT_ALL": "1",
                    "REQUEST_METHOD": self.command,
                    "PATH_INFO": path,
                    "QUERY_STRING": query,
                    "CONTENT_TYPE": self.headers.get("Content-Type", ""),
                    "CONTENT_LENGTH": str(len(body)),
                    "REMOTE_USER": auth[0] if auth else "",
                    "REMOTE_ADDR": "127.0.0.1",
                    "GIT_PROTOCOL": self.headers.get("Git-Protocol", ""),
                }
            )
            proc = subprocess.run(
                ["git", "http-backend"],
                input=body,
                env=cgi_env,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            raw = proc.stdout
            head, _, payload = raw.partition(b"\r\n\r\n")
            status = 200
            headers: list[tuple[str, str]] = []
            for line in head.split(b"\r\n"):
                if not line:
                    continue
                key, _, value = line.decode("latin-1").partition(":")
                value = value.strip()
                if key.lower() == "status":
                    status = int(value.split()[0])
                else:
                    headers.append((key, value))
            self.send_response(status)
            for key, value in headers:
                self.send_header(key, value)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self) -> None:  # noqa: N802
            self._run_backend(b"")

        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length", "0"))
            self._run_backend(self.rfile.read(length))

        def log_message(self, *_args) -> None:
            pass

    return Handler


def serve(
    project_root: Path, env: dict[str, str], auth: tuple[str, str] | None
) -> ThreadingHTTPServer:
    handler = make_handler(project_root, env, auth)
    return ThreadingHTTPServer(("127.0.0.1", 0), handler)
