#!/usr/bin/env python3
"""Local fake SMTP server over STARTTLS, for testing compose/reply/forward send without a real account.

Implements what src/email/smtp.rs sends: EHLO, STARTTLS then a second EHLO (RFC 3207), AUTH LOGIN
(base64 username/password), MAIL FROM/RCPT TO(s)/DATA with dot-stuffing, QUIT (reply ignored by the
client, so this server need not be clever about it). Implicit TLS (port 465) is not implemented: the
client only takes that path when `config.smtp_port == 465` literally, which would mean binding the
real privileged port 465 to exercise it - not worth it when STARTTLS (what every configured account
without an explicit `SMTP_PORT=465` gets) already covers the shared post-TLS code path.

Every accepted message is appended to `messages.log` as one JSON line ({"mail_from", "rcpt_to",
"headers", "body"}) for scenario assertions. The mode file (`Smtp.set_mode`) can switch AUTH LOGIN or
every RCPT TO to a rejection, to exercise the "Send failed: ..." UI path. TLS uses the same throwaway
CA/leaf approach as fakeimap.py (reused via `fakeimap.make_certs`); the binary trusts it through
`SSL_CERT_FILE`.

Run as `fakesmtp.py DIR`: serves on 127.0.0.1, writes the port to DIR/port.
Usage from the driver: docs/TUI_DRIVER.md, docs/TUI_FAKES.md ("Fake SMTP").
"""
from __future__ import annotations

import base64
import json
import os
import re
import socket
import ssl
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fakeimap  # noqa: E402 - reuses its throwaway CA/leaf cert generation

USER, PASSWORD = "tester@example.com", "secret"


class Smtp:
    """Handle on a running fake SMTP server's state directory."""

    def __init__(self, d: Path):
        self.d = d

    def set_mode(self, mode: str) -> None:
        """`ok` (default), `auth_fail` (AUTH LOGIN rejected), or `reject_recipient` (every RCPT TO rejected)."""
        (self.d / "mode").write_text(mode)

    def messages(self) -> list[dict]:
        try:
            return [json.loads(l) for l in (self.d / "messages.log").read_text().splitlines()]
        except OSError:
            return []


def _mode(d: Path) -> str:
    return (d / "mode").read_text().strip() if (d / "mode").exists() else "ok"


class Handler:
    def __init__(self, conn: socket.socket, ctx: ssl.SSLContext, d: Path):
        self.conn, self.ctx, self.d = conn, ctx, d
        self.rfile = conn.makefile("rb")

    def w(self, line: str) -> None:
        self.conn.sendall(line.encode())

    def log(self, line: str) -> None:
        with open(self.d / "commands.log", "a") as f:
            f.write(line + "\n")

    def readline(self) -> str | None:
        raw = self.rfile.readline()
        return raw.decode(errors="replace").rstrip("\r\n") if raw else None

    def run(self) -> None:
        try:
            self._run()
        finally:
            try:
                self.conn.close()
            except OSError:
                pass

    def _run(self) -> None:
        mode = _mode(self.d)
        self.w("220 fakesmtp ready\r\n")

        line = self.readline()
        if line is None or not line.upper().startswith("EHLO"):
            return
        self.log(line)
        self.w("250-fakesmtp\r\n250 STARTTLS\r\n")

        line = self.readline()
        if line is None or line.upper() != "STARTTLS":
            return
        self.log(line)
        self.w("220 go ahead\r\n")

        tls = self.ctx.wrap_socket(self.conn, server_side=True)
        self.conn, self.rfile = tls, tls.makefile("rb")

        line = self.readline()
        if line is None or not line.upper().startswith("EHLO"):
            return
        self.log(line)
        self.w("250 fakesmtp\r\n")

        line = self.readline()
        if line is None or line.upper() != "AUTH LOGIN":
            return
        self.log(line)
        self.w("334 VXNlcm5hbWU6\r\n")

        user_b64 = self.readline()
        self.log("<username>")
        self.w("334 UGFzc3dvcmQ6\r\n")

        pass_b64 = self.readline()
        self.log("<password>")
        user = base64.b64decode(user_b64 or "").decode(errors="replace")
        pw = base64.b64decode(pass_b64 or "").decode(errors="replace")
        if mode == "auth_fail" or (user, pw) != (USER, PASSWORD):
            self.w("535 authentication failed\r\n")
            return
        self.w("235 authentication successful\r\n")

        line = self.readline()
        if line is None:
            return
        self.log(line)
        m = re.match(r"(?i)MAIL FROM:<(.*)>", line)
        if not m:
            self.w("500 expected MAIL FROM\r\n")
            return
        mail_from = m.group(1)
        self.w("250 OK\r\n")

        rcpt_to: list[str] = []
        while True:
            line = self.readline()
            if line is None:
                return
            self.log(line)
            if line.upper().startswith("RCPT TO"):
                if mode == "reject_recipient":
                    self.w("550 recipient rejected\r\n")
                    continue
                m = re.match(r"(?i)RCPT TO:<(.*)>", line)
                rcpt_to.append(m.group(1) if m else "")
                self.w("250 OK\r\n")
            elif line.upper() == "DATA":
                break
            else:
                self.w("500 unexpected command\r\n")
                return

        self.w("354 start mail input, end with <CRLF>.<CRLF>\r\n")
        raw_lines = []
        while True:
            raw = self.rfile.readline()
            if not raw:
                return
            text = raw.decode(errors="replace")
            if text in (".\r\n", ".\n"):
                break
            raw_lines.append(text[1:] if text.startswith("..") else text)
        self._record(mail_from, rcpt_to, "".join(raw_lines))
        self.w("250 OK: queued\r\n")

        line = self.readline()
        if line is not None:
            self.log(line)
        self.w("221 bye\r\n")

    def _record(self, mail_from: str, rcpt_to: list[str], raw: str) -> None:
        head, _, body = raw.partition("\r\n\r\n")
        headers: dict[str, str] = {}
        for hline in head.split("\r\n"):
            k, sep, v = hline.partition(":")
            if sep:
                headers[k.strip()] = v.strip()
        entry = {"mail_from": mail_from, "rcpt_to": rcpt_to, "headers": headers, "body": body.rstrip("\r\n")}
        with open(self.d / "messages.log", "a") as f:
            f.write(json.dumps(entry) + "\n")


def serve(d: Path) -> None:
    fakeimap.make_certs(d)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(d / "srv.pem", d / "srv.key")
    srv = socket.create_server(("127.0.0.1", 0))
    (d / "port.tmp").write_text(str(srv.getsockname()[1]))
    (d / "port.tmp").replace(d / "port")

    def one(raw: socket.socket) -> None:
        try:
            Handler(raw, ctx, d).run()
        except (OSError, ssl.SSLError) as e:
            with open(d / "commands.log", "a") as f:
                f.write(f"# connection error: {e}\n")

    while True:
        conn, _ = srv.accept()
        threading.Thread(target=one, args=(conn,), daemon=True).start()


def start(d: Path, timeout: float = 15) -> subprocess.Popen:
    """Launch `serve(d)` as a detached process; returns once it listens. Caller kills it."""
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
    raise RuntimeError(f"fakesmtp did not start: {(d / 'server.out').read_text()[-400:]}")


def env_for(d: Path) -> dict[str, str]:
    """Env that points the binary at this server only (single legacy "default" account)."""
    return {"TRIPTYCH_EMAIL_ENABLED": "true", "SMTP_SERVER": "localhost", "SMTP_PORT": (d / "port").read_text(),
            "SMTP_USERNAME": USER, "SMTP_PASSWORD": PASSWORD, "SSL_CERT_FILE": str(d / "ca.pem")}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    os.chdir(sys.argv[1])
    serve(Path(sys.argv[1]).resolve())
