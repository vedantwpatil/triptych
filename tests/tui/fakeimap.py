#!/usr/bin/env python3
"""Local fake IMAP4rev1 server over TLS, for testing `triptych email sync` without a real account.

Implements what src/email/client.rs uses: LOGIN, SELECT (UIDVALIDITY; INBOX or any other name —
see below), LIST (Slice 16: `\\Noselect`-aware, see below), UID SEARCH (ALL | UID n:*), UID FETCH
(RFC822.SIZE | RFC822 | RFC822.HEADER), UID STORE (+/-FLAGS(.SILENT) (\\Deleted)), EXPUNGE,
UID MOVE / UID COPY (archive), IDLE/DONE (RFC 2177: `+ idling` continuation, then an unsolicited
`* n EXISTS` the moment the mailbox's message count changes, held open until the client sends
`DONE`), LOGOUT, plus CAPABILITY/NOOP.
`Mailbox.disable_move()` makes UID MOVE answer BAD, for testing the client's
COPY+STORE+EXPUNGE fallback. SELECT of a non-INBOX name succeeds once a prior MOVE/COPY has
tagged something for it in `box["archived"]`, *or* it was registered empty via
`Mailbox.add_folder` (otherwise NO [NONEXISTENT], same as an account that never archives
anything); once it exists, that name is a second mailbox backed by the matching `box["archived"]`
entries. This is what lets a folder-browsing test (Slice 13) resync a message out of "Archive"
after archiving it, without changing sync-cursor behavior for every other account that never does.
LIST (Slice 16) enumerates INBOX plus every `add_folder`-registered or `archived`-tagged name,
flagging any in `box["noselect"]` with `\\Noselect` — real reference/pattern filtering isn't
implemented since `client.rs`'s `list_folders_inner` only ever sends `LIST "" *`.
The mailbox is a JSON file (`Mailbox`), re-read on every command, so tests change it between syncs.
`Mailbox.add(..., attachment=True)` builds a multipart/mixed message with one `application/pdf`
part (`report-<uid>.pdf`), for exercising attachment extraction/save without a real MIME payload.
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
                 pad: int = 0, age_minutes: int = 0, attachment: bool = False) -> str:
    date = format_datetime(datetime.now(timezone.utc) - timedelta(minutes=age_minutes + uid))
    body = body or f"Body of {subject}."
    if pad:
        body += "\r\n" + ("x" * 76 + "\r\n") * (pad // 78 + 1)
    headers = (f"From: {sender}\r\nTo: tester@example.com\r\nSubject: {subject}\r\nDate: {date}\r\n"
               f"Message-ID: <fake-{uid}-{abs(hash(subject)) % 10**8}@fakeimap>\r\n")
    if not attachment:
        return headers + f"Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    boundary = f"BOUNDARY{uid}"
    return (headers + f"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
            f"--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
            f"--{boundary}\r\nContent-Type: application/pdf\r\n"
            f"Content-Disposition: attachment; filename=\"report-{uid}.pdf\"\r\n"
            f"Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n"
            f"--{boundary}--\r\n")


class Mailbox:
    """JSON-backed INBOX: {"uidvalidity", "next_uid", "messages": [{"uid", "raw"}],
    "archived": [{"uid", "raw", "folder"}]}. `archived` doubles as every other SELECTable
    mailbox's storage, partitioned by its `folder` tag (see `Handler._mailbox_messages`).
    Safe to edit while serving."""

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

    def disable_move(self) -> None:
        """Makes `UID MOVE` answer BAD, so the client falls back to COPY+STORE+EXPUNGE."""
        box = self.load()
        box["no_move"] = True
        self.save(box)

    def add_folder(self, name: str, noselect: bool = False) -> None:
        """Registers a selectable server folder with no messages yet (e.g. "Sent", "Drafts"), so
        LIST reports it before anything's ever been synced into or archived to it — unlike
        `archived`'s folders, which only "exist" once a message has actually been moved/copied
        there. `noselect=True` marks it `\\Noselect` in the LIST response (a hierarchy-only node),
        for asserting the client actually drops it (Slice 16: folder browsing)."""
        box = self.load()
        folders = box.setdefault("folders", [])
        if name not in folders:
            folders.append(name)
        if noselect:
            box.setdefault("noselect", []).append(name)
        self.save(box)

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
        self.folder = "INBOX"

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
            name = (_args(arg) or ["INBOX"])[0]
            box = self.box.load()
            # A non-INBOX name only "exists" once a MOVE/COPY has tagged something for it, or it
            # was registered empty via `Mailbox.add_folder` — an account that does neither must
            # keep seeing this SELECT fail, so its sync cursor table stays exactly as before
            # Slice 13 (folder browsing).
            known = name.upper() == "INBOX" or any(
                m["folder"] == name for m in box.get("archived", [])
            ) or name in box.get("folders", [])
            if not known:
                self.w(f"{tag} NO [NONEXISTENT] no such mailbox\r\n")
                return True
            self.folder = name
            msgs = self._mailbox_messages(box)
            self.selected = True
            self.w(f"* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)\r\n* {len(msgs)} EXISTS\r\n"
                   f"* 0 RECENT\r\n* OK [UIDVALIDITY {box['uidvalidity']}] UIDs valid\r\n"
                   f"* OK [UIDNEXT {box['next_uid']}] next\r\n{tag} OK [READ-WRITE] SELECT completed\r\n")
        elif cmd == "LIST":
            self.list_(tag, arg)
        elif not self.selected:
            self.w(f"{tag} BAD no mailbox selected\r\n")
        elif cmd == "UID":
            self.uid(tag, arg)
        elif cmd == "EXPUNGE":
            self.expunge(tag)
        elif cmd == "IDLE":
            self.idle(tag)
        else:
            self.w(f"{tag} BAD unsupported {cmd}\r\n")
        return True

    def list_(self, tag: str, arg: str) -> None:
        """`LIST "" *` (or any reference/pattern — only `*` is actually sent by `client.rs`'s
        `list_folders_inner`, so neither is filtered on here): enumerates INBOX, every folder
        registered via `Mailbox.add_folder`, and every distinct `archived` destination folder
        (e.g. Archive, once something's been moved there). A name in `box["noselect"]` gets the
        `\\Noselect` attribute, matching a real server's hierarchy-only nodes, so a folder-browsing
        test can assert the client actually drops it (Slice 16)."""
        box = self.box.load()
        names = {"INBOX"} | {m["folder"] for m in box.get("archived", [])} | set(box.get("folders", []))
        noselect = set(box.get("noselect", []))
        for name in sorted(names, key=str.upper):
            attrs = r"\Noselect" if name in noselect else ""
            self.w(f'* LIST ({attrs}) "/" "{name}"\r\n')
        self.w(f"{tag} OK LIST completed\r\n")

    def _mailbox_messages(self, box: dict) -> list[dict]:
        """Messages visible in `self.folder`: `box["messages"]` for INBOX, else the subset of
        `box["archived"]` tagged for that destination folder (Slice 13 folder browsing)."""
        if self.folder.upper() == "INBOX":
            return box["messages"]
        return [m for m in box.get("archived", []) if m["folder"] == self.folder]

    def _remove_messages(self, box: dict, uids: set[int]) -> None:
        """Removes `uids` from wherever `self.folder` currently keeps them (in place on `box`)."""
        if self.folder.upper() == "INBOX":
            box["messages"] = [m for m in box["messages"] if m["uid"] not in uids]
        else:
            box["archived"] = [
                m for m in box.get("archived", [])
                if not (m["folder"] == self.folder and m["uid"] in uids)
            ]

    def uid(self, tag: str, arg: str) -> None:
        sub, _, rest = arg.partition(" ")
        msgs = self._mailbox_messages(self.box.load())
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
        elif sub.upper() == "STORE":
            spec, _, flags_arg = rest.partition(" ")
            m = re.fullmatch(r"(?i)([+-]?)FLAGS(\.SILENT)?\s*\((.*)\)", flags_arg.strip())
            if not m or "deleted" not in m.group(3).lower():
                self.w(f"{tag} BAD unsupported store {flags_arg}\r\n")
                return
            sign = m.group(1) or "+"
            box = self.box.load()
            targets = _uid_set(spec, uids)
            for msg in self._mailbox_messages(box):
                if msg["uid"] in targets:
                    if sign == "-":
                        msg.pop("deleted", None)
                    else:
                        msg["deleted"] = True
            self.box.save(box)
            self.w(f"{tag} OK STORE completed\r\n")
        elif sub.upper() == "MOVE":
            box = self.box.load()
            if box.get("no_move"):
                self.w(f"{tag} BAD MOVE not supported\r\n")
                return
            spec, _, dest_arg = rest.partition(" ")
            dest = (_args(dest_arg) or [""])[0]
            targets = set(_uid_set(spec, uids))
            moved = [m for m in self._mailbox_messages(box) if m["uid"] in targets]
            self._remove_messages(box, targets)
            box.setdefault("archived", []).extend(
                {"uid": m["uid"], "raw": m["raw"], "folder": dest} for m in moved
            )
            self.box.save(box)
            self.w(f"{tag} OK MOVE completed\r\n")
        elif sub.upper() == "COPY":
            spec, _, dest_arg = rest.partition(" ")
            dest = (_args(dest_arg) or [""])[0]
            box = self.box.load()
            targets = set(_uid_set(spec, uids))
            copied = [m for m in self._mailbox_messages(box) if m["uid"] in targets]
            box.setdefault("archived", []).extend(
                {"uid": m["uid"], "raw": m["raw"], "folder": dest} for m in copied
            )
            self.box.save(box)
            self.w(f"{tag} OK COPY completed\r\n")
        else:
            self.w(f"{tag} BAD unsupported UID {sub}\r\n")

    def idle(self, tag: str) -> None:
        """RFC 2177: `+` continuation, then block until either `self.folder`'s message count
        changes (push one unsolicited `* n EXISTS`, matching real servers' "just notify, don't
        say what changed" behavior — the client is expected to run a normal fetch afterward) or
        the client sends a bare `DONE` line, whichever first. A side thread polls the mailbox file
        every 0.15s (it's JSON, written by other test code, not something to block a read on);
        the main thread keeps a plain blocking `readline()` waiting for `DONE`, so the socket
        never needs a read timeout (which does not mix well with a buffered socket file object)."""
        self.w("+ idling\r\n")
        baseline = len(self._mailbox_messages(self.box.load()))
        stop, wlock = threading.Event(), threading.Lock()

        def poll() -> None:
            while not stop.is_set():
                current = len(self._mailbox_messages(self.box.load()))
                if current != baseline:
                    with wlock:
                        try:
                            self.w(f"* {current} EXISTS\r\n")
                        except OSError:
                            pass
                    return
                stop.wait(0.15)

        poller = threading.Thread(target=poll, daemon=True)
        poller.start()
        try:
            while line := self.rfile.readline():
                text = line.decode(errors="replace").rstrip("\r\n")
                self.log(text)
                if text.strip().upper() == "DONE":
                    with wlock:
                        self.w(f"{tag} OK IDLE terminated\r\n")
                    return
        finally:
            stop.set()
            poller.join(timeout=1)

    def expunge(self, tag: str) -> None:
        """Removes every `\\Deleted`-flagged message from `self.folder`, emitting `* n EXPUNGE` per
        RFC 3501: sequence numbers shift down as earlier removals in the same response take
        effect, so the n-th removed message (in ascending original order) is reported as
        `original_seq - n` (n 0-based)."""
        box = self.box.load()
        msgs = self._mailbox_messages(box)
        removed_at = [i + 1 for i, m in enumerate(msgs) if m.get("deleted")]
        removed_uids = {m["uid"] for m in msgs if m.get("deleted")}
        self._remove_messages(box, removed_uids)
        self.box.save(box)
        out = "".join(f"* {seq - i} EXPUNGE\r\n" for i, seq in enumerate(removed_at))
        self.w(f"{out}{tag} OK EXPUNGE completed\r\n")


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
