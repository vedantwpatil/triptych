#!/usr/bin/env python3
"""Local fake IMAP4rev1 server over TLS, for testing `triptych email sync` without a real account.

Implements what src/email/client.rs uses: LOGIN, SELECT (UIDVALIDITY), UID SEARCH (ALL | UID n:*),
UID FETCH (RFC822.SIZE | RFC822 | RFC822.HEADER), LOGOUT, plus CAPABILITY/NOOP.
The mailbox is a JSON file (`Mailbox`), re-read on every command, so tests change it between syncs.
Every command is appended to `commands.log` for assertions. TLS uses a throwaway CA made with the
openssl CLI; the binary trusts it through `SSL_CERT_FILE`, which also stops it trusting real CAs.

Run as `fakeimap.py DIR`: serves DIR/mailbox.json on 127.0.0.1, writes the port to DIR/port.
Usage from the driver: docs/TUI_DRIVER.md ("Email and IMAP").
"""
from __future__ import annotations

import json
import os
import re
import shutil
import socket
import ssl
import subprocess
import sys
import threading
import time
from email.utils import format_datetime
from datetime import datetime, timedelta, timezone
from pathlib import Path

USER, PASSWORD = "tester", "secret"


def _openssl() -> str:
    for cand in ("/opt/homebrew/bin/openssl", "/usr/local/bin/openssl", shutil.which("openssl")):
        if cand and Path(cand).exists():
            return cand
    raise SystemExit("fakeimap: openssl CLI not found")


def make_certs(d: Path) -> None:
    """CA (ca.pem) plus a localhost leaf (srv.pem/srv.key) signed by it. rustls rejects self-signed leaves."""
    if (d / "srv.pem").exists():
        return
    o = _openssl()
    run = lambda *a: subprocess.run([o, *a], cwd=d, check=True, capture_output=True)
    ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"]
    run("req", "-x509", *ec, "-keyout", "ca.key", "-out", "ca.pem", "-days", "3650", "-subj", "/CN=tuidrive test CA",
        "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign")
    run("req", *ec, "-keyout", "srv.key", "-out", "srv.csr", "-subj", "/CN=localhost")
    (d / "srv.ext").write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\n"
                               "extendedKeyUsage=serverAuth\n")
    run("x509", "-req", "-in", "srv.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-out", "srv.pem",
        "-days", "3650", "-extfile", "srv.ext")


def make_message(uid: int, subject: str, sender: str = "Alice Example <alice@example.com>", body: str = "",
                 pad: int = 0, age_minutes: int = 0) -> str:
    date = format_datetime(datetime.now(timezone.utc) - timedelta(minutes=age_minutes + uid))
    body = body or f"Body of {subject}."
    if pad:
        body += "\r\n" + ("x" * 76 + "\r\n") * (pad // 78 + 1)
    return (f"From: {sender}\r\nTo: tester@example.com\r\nSubject: {subject}\r\nDate: {date}\r\n"
            f"Message-ID: <fake-{uid}-{abs(hash(subject)) % 10**8}@fakeimap>\r\n"
            f"Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n")


class Mailbox:
    """JSON-backed INBOX: {"uidvalidity", "next_uid", "messages": [{"uid", "raw"}]}. Safe to edit while serving."""

    def __init__(self, d: Path):
        self.path = d / "mailbox.json"

    def load(self) -> dict:
        try:
            return json.loads(self.path.read_text())
        except (OSError, json.JSONDecodeError):
            return {"uidvalidity": 1, "next_uid": 1, "messages": []}

    def save(self, box: dict) -> None:
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(box))
        tmp.replace(self.path)

    def add(self, subject: str = "", count: int = 1, big: bool = False, **kw) -> list[int]:
        """Append `count` messages; `big` pads each past the client's 1 MiB header-only threshold."""
        box, uids = self.load(), []
        for i in range(count):
            uid = box["next_uid"]
            subj = subject or f"Fake message {uid}"
            if count > 1 and subject:
                subj = f"{subject} {i + 1}"
            raw = make_message(uid, subj, pad=1_200_000 if big else 0, **kw)
            box["messages"].append({"uid": uid, "raw": raw})
            box["next_uid"] = uid + 1
            uids.append(uid)
        self.save(box)
        return uids

    def reset_uids(self, uidvalidity: int) -> None:
        """Simulate a server-side mailbox rebuild: new UIDVALIDITY, messages renumbered from 1."""
        box = self.load()
        for i, m in enumerate(box["messages"], 1):
            m["uid"] = i
        box.update(uidvalidity=uidvalidity, next_uid=len(box["messages"]) + 1)
        self.save(box)


def _uid_set(spec: str, uids: list[int]) -> list[int]:
    """RFC 3501 sequence set over UIDs. `n:*` always includes the highest UID, even when n is past it."""
    top, out = max(uids, default=0), set()
    for part in spec.split(","):
        a, _, b = part.partition(":")
        lo = top if a == "*" else int(a)
        hi = (top if b == "*" else int(b)) if b else lo
        lo, hi = min(lo, hi), max(lo, hi)
        out.update(u for u in uids if lo <= u <= hi)
    return sorted(out)


def _args(s: str) -> list[str]:
    return [a if a is not None and a != "" else b for a, b in re.findall(r'"((?:[^"\\]|\\.)*)"|(\S+)', s)]


class Handler:
    def __init__(self, conn: ssl.SSLSocket, d: Path):
        self.conn, self.d, self.box = conn, d, Mailbox(d)
        self.rfile = conn.makefile("rb")
        self.authed = self.selected = False

    def w(self, data: str | bytes) -> None:
        self.conn.sendall(data.encode() if isinstance(data, str) else data)

    def log(self, line: str) -> None:
        with open(self.d / "commands.log", "a") as f:
            f.write(re.sub(r"(LOGIN \S+) .*", r"\1 ***", line, flags=re.I) + "\n")

    def run(self) -> None:
        self.w("* OK [CAPABILITY IMAP4rev1] fakeimap ready\r\n")
        while line := self.rfile.readline():
            line = line.decode(errors="replace").rstrip("\r\n")
            self.log(line)
            tag, _, rest = line.partition(" ")
            cmd, _, arg = rest.partition(" ")
            if not self.dispatch(tag, cmd.upper(), arg):
                return

    def dispatch(self, tag: str, cmd: str, arg: str) -> bool:
        if cmd == "CAPABILITY":
            self.w(f"* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n")
        elif cmd == "NOOP":
            self.w(f"{tag} OK done\r\n")
        elif cmd == "LOGOUT":
            self.w(f"* BYE bye\r\n{tag} OK done\r\n")
            return False
        elif cmd == "LOGIN":
            user, pw = (_args(arg) + ["", ""])[:2]
            if (user, pw) == (USER, PASSWORD):
                self.authed = True
                self.w(f"{tag} OK LOGIN completed\r\n")
            else:
                self.w(f"{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
        elif not self.authed:
            self.w(f"{tag} BAD not authenticated\r\n")
        elif cmd in ("SELECT", "EXAMINE"):
            if (_args(arg) or [""])[0].upper() != "INBOX":
                self.w(f"{tag} NO [NONEXISTENT] no such mailbox\r\n")
                return True
            box = self.box.load()
            self.selected = True
            self.w(f"* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)\r\n* {len(box['messages'])} EXISTS\r\n"
                   f"* 0 RECENT\r\n* OK [UIDVALIDITY {box['uidvalidity']}] UIDs valid\r\n"
                   f"* OK [UIDNEXT {box['next_uid']}] next\r\n{tag} OK [READ-WRITE] SELECT completed\r\n")
        elif not self.selected:
            self.w(f"{tag} BAD no mailbox selected\r\n")
        elif cmd == "UID":
            self.uid(tag, arg)
        else:
            self.w(f"{tag} BAD unsupported {cmd}\r\n")
        return True

    def uid(self, tag: str, arg: str) -> None:
        sub, _, rest = arg.partition(" ")
        msgs = self.box.load()["messages"]
        uids = [m["uid"] for m in msgs]
        if sub.upper() == "SEARCH":
            m = re.fullmatch(r"(?i)(ALL|UID (\S+))", rest.strip())
            if not m:
                self.w(f"{tag} BAD unsupported search {rest}\r\n")
                return
            hits = uids if m.group(1).upper() == "ALL" else _uid_set(m.group(2), uids)
            self.w(f"* SEARCH{''.join(f' {u}' for u in hits)}\r\n{tag} OK SEARCH completed\r\n")
        elif sub.upper() == "FETCH":
            spec, _, items = rest.partition(" ")
            items = items.strip("()").upper()
            for u in _uid_set(spec, uids):
                seq = uids.index(u) + 1
                raw = next(m["raw"] for m in msgs if m["uid"] == u).encode()
                if items == "RFC822.SIZE":
                    self.w(f"* {seq} FETCH (UID {u} RFC822.SIZE {len(raw)})\r\n")
                elif items in ("RFC822", "RFC822.HEADER"):
                    data = raw if items == "RFC822" else raw.split(b"\r\n\r\n", 1)[0] + b"\r\n\r\n"
                    self.w(f"* {seq} FETCH (UID {u} {items} {{{len(data)}}}\r\n".encode() + data + b")\r\n")
                else:
                    self.w(f"{tag} BAD unsupported fetch {items}\r\n")
                    return
            self.w(f"{tag} OK FETCH completed\r\n")
        else:
            self.w(f"{tag} BAD unsupported UID {sub}\r\n")


def serve(d: Path) -> None:
    make_certs(d)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(d / "srv.pem", d / "srv.key")
    srv = socket.create_server(("127.0.0.1", 0))
    (d / "port.tmp").write_text(str(srv.getsockname()[1]))
    (d / "port.tmp").replace(d / "port")

    def one(raw: socket.socket) -> None:
        try:
            with ctx.wrap_socket(raw, server_side=True) as conn:
                Handler(conn, d).run()
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
    raise RuntimeError(f"fakeimap did not start: {(d / 'server.out').read_text()[-400:]}")


def env_for(d: Path) -> dict[str, str]:
    """Env that points the binary at this server only (single legacy "default" account)."""
    return {"TRIPTYCH_EMAIL_ENABLED": "true", "IMAP_SERVER": "localhost", "IMAP_PORT": (d / "port").read_text(),
            "IMAP_USERNAME": USER, "IMAP_PASSWORD": PASSWORD, "IMAP_FOLDER": "INBOX",
            "SSL_CERT_FILE": str(d / "ca.pem")}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    os.chdir(sys.argv[1])
    serve(Path(sys.argv[1]).resolve())
