#!/usr/bin/env python3
"""Local fake Ollama for the TUI driver, so summary scenarios never depend on a real model.

Serves what src/nlp/ollama_client.rs calls: GET /api/tags, GET /api/ps (the model counts as loaded),
POST /api/generate. A prompt that asks for a summary gets "Fake summary of: <subject>"; an empty
prompt (the model warm-up) gets an empty reply; anything else (a task parse) gets `{}`, which the
parser rejects, so it falls back to regex. The mode file (`Ollama.set_mode`) can switch generate to
HTTP 500 while running. Every request is appended to DIR/requests.log as one JSON line.

Run as `fakeollama.py DIR`: serves on 127.0.0.1, writes the port to DIR/port.
Usage from the driver: docs/TUI_DRIVER.md ("Ollama").
"""
from __future__ import annotations

import json
import re
import socket
import socketserver
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MODEL = "qwen2.5:7b"


class Ollama:
    """Handle on a running (or deliberately absent) fake server's state directory."""

    def __init__(self, d: Path):
        self.d = d

    def set_mode(self, mode: str) -> None:
        """`ok` (default) or `error` (generate returns HTTP 500)."""
        (self.d / "mode").write_text(mode)

    def requests(self) -> list[dict]:
        try:
            return [json.loads(l) for l in (self.d / "requests.log").read_text().splitlines()]
        except OSError:
            return []

    def summary_requests(self) -> list[dict]:
        return [r for r in self.requests() if r["kind"] == "summary"]


def _kind(prompt: str) -> str:
    if not prompt:
        return "warm"
    return "summary" if prompt.startswith("Summarize the email") else "parse"


def make_handler(d: Path):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def _send(self, code: int, body: dict) -> None:
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _log(self, kind: str) -> None:
            with open(d / "requests.log", "a") as f:
                f.write(json.dumps({"path": self.path, "kind": kind}) + "\n")

        def do_GET(self):
            if self.path == "/api/tags":
                self._send(200, {"models": [{"name": MODEL}]})
            elif self.path == "/api/ps":
                self._send(200, {"models": [{"name": MODEL}]})
            else:
                self._send(404, {"error": "not found"})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
            prompt = body.get("prompt", "")
            kind = _kind(prompt)
            self._log(kind)
            mode = (d / "mode").read_text().strip() if (d / "mode").exists() else "ok"
            if self.path != "/api/generate":
                self._send(404, {"error": "not found"})
            elif mode == "error" and kind != "warm":
                self._send(500, {"error": "fake failure"})
            elif kind == "summary":
                subject = re.search(r"^Subject: (.*)$", prompt, re.M)
                self._send(200, {"response": f"Fake summary of: {subject.group(1) if subject else '?'}"})
            elif kind == "warm":
                self._send(200, {"response": "", "done": True})
            else:
                self._send(200, {"response": "{}"})

    return Handler


class Server(ThreadingHTTPServer):
    def server_bind(self) -> None:
        # HTTPServer.server_bind reverse-resolves the address (getfqdn), which can stall for many seconds.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = "localhost", self.server_address[1]


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start(d: Path, timeout: float = 10) -> subprocess.Popen:
    """Launch the server as a detached process; returns once it listens. Caller kills it."""
    d.mkdir(parents=True, exist_ok=True)
    (d / "port").unlink(missing_ok=True)
    proc = subprocess.Popen([sys.executable, __file__, str(d)], stdout=open(d / "server.out", "a"),
                            stderr=subprocess.STDOUT, start_new_session=True)
    end = time.time() + timeout
    while time.time() < end and proc.poll() is None:
        if (d / "port").exists():
            return proc
        time.sleep(0.05)
    proc.kill()
    raise RuntimeError(f"fakeollama did not start: {(d / 'server.out').read_text()[-400:]}")


def env_for(d: Path) -> dict[str, str]:
    """Env that points the binary at this server (or, with `down`, at a port nothing listens on)."""
    return {"TRIPTYCH_OLLAMA_URL": f"http://127.0.0.1:{(d / 'port').read_text()}"}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    d = Path(sys.argv[1]).resolve()
    srv = Server(("127.0.0.1", 0), make_handler(d))
    (d / "port.tmp").write_text(str(srv.server_address[1]))
    (d / "port.tmp").replace(d / "port")
    srv.serve_forever()
