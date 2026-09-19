#!/usr/bin/env python3
"""Headless pty driver for the Triptych TUI. Usage and design: docs/TUI_DRIVER.md.

Library layer (used by tools/tui_suite.py): Sandbox, Term, parse_keys.
CLI layer: `start` spawns a detached server that owns a pty + terminal emulator, so separate
shell invocations (`send`, `screen`, `wait`, `db`, `cli`, `stop`) can drive one live TUI session.
Every session runs against a throwaway sandbox dir; it never touches the real todo.db or socket.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
import pty
import re
import select
import shlex
import shutil
import signal
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENV = ROOT / "target" / "tuidrive-venv"

try:
    import pyte
except ImportError:  # first run: build a private venv under target/ (gitignored) and re-exec
    if os.environ.get("TUIDRIVE_BOOTSTRAPPED"):
        sys.exit("tuidrive: pyte still missing after bootstrap")
    venv_py = VENV / "bin" / "python"
    if not venv_py.exists():
        subprocess.check_call([sys.executable, "-m", "venv", str(VENV)])
    subprocess.check_call([str(venv_py), "-m", "pip", "install", "-q", "--disable-pip-version-check", "pyte"])
    os.environ["TUIDRIVE_BOOTSTRAPPED"] = "1"
    os.execv(str(venv_py), [str(venv_py), *sys.argv])

BIN = Path(os.environ.get("TRIPTYCH_BIN") or ROOT / "target" / "debug" / "Triptych")
HOME = Path(os.environ.get("TUIDRIVE_HOME") or f"/tmp/tuidrive-{os.getuid()}")
PROTECTED_ENV = {"DATABASE_URL", "TRIPTYCH_SOCKET_PATH", "TRIPTYCH_LOG_PATH"}
ALT_ON = b"\x1b[?1049h"
CURSOR_BG = "7f7f7f"  # calendar selection highlight colour

NAMED_KEYS = {
    "ENTER": "\r", "ESC": "\x1b", "TAB": "\t", "BTAB": "\x1b[Z", "BS": "\x7f", "SPACE": " ",
    "UP": "\x1b[A", "DOWN": "\x1b[B", "RIGHT": "\x1b[C", "LEFT": "\x1b[D",
    "HOME": "\x1b[H", "END": "\x1b[F", "PGUP": "\x1b[5~", "PGDN": "\x1b[6~", "DEL": "\x1b[3~",
}


def parse_keys(tokens: list[str]) -> list[str]:
    """NAME (see NAMED_KEYS), C-x, t:literal text, or a single character -> writes, one per key."""
    out: list[str] = []
    for tok in tokens:
        if tok in NAMED_KEYS:
            out.append(NAMED_KEYS[tok])
        elif tok.startswith("t:"):
            out.extend(tok[2:])
        elif re.fullmatch(r"C-[a-z]", tok):
            out.append(chr(ord(tok[2]) - 96))
        elif len(tok) == 1:
            out.append(tok)
        else:
            raise ValueError(f"bad key token {tok!r}: use NAME ({', '.join(NAMED_KEYS)}), C-x, t:text or one char")
    return out


class CliResult:
    def __init__(self, rc: int, out: str, err: str):
        self.rc, self.out, self.err = rc, out, err

    @property
    def clean_out(self) -> str:
        return "\n".join(l for l in self.out.splitlines() if "NLP parsing ready" not in l and "Ollama unavailable" not in l)

    @property
    def clean_err(self) -> str:
        return "\n".join(l for l in self.err.splitlines() if not (l.startswith("[Migration]") or l.startswith("  ✓") or l.startswith("  Rebuilding")))


class Sandbox:
    """Throwaway dir holding an isolated DB, daemon socket and log. The only way the driver runs the binary."""

    def __init__(self, name: str):
        self.name = name
        self.dir = HOME / name
        self.db_path = self.dir / "todo.db"
        self.sock_path = self.dir / "t.sock"
        self.bg: list[subprocess.Popen] = []

    @classmethod
    def create(cls, name: str) -> "Sandbox":
        sb = cls(name)
        if sb.is_live():
            raise SystemExit(f"session {name!r} already running (tuidrive -s {name} stop)")
        if sb.dir.exists():
            shutil.rmtree(sb.dir)
        sb.dir.mkdir(parents=True)
        if len(str(sb.dir / "ctl.sock")) > 100:
            raise SystemExit("session path too long for a unix socket; set TUIDRIVE_HOME to a shorter dir")
        return sb

    def is_live(self) -> bool:
        return (self.dir / "ctl.sock").exists() and request(self.name, {"op": "status"}, quiet=True) is not None

    def env(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {k: v for k, v in os.environ.items() if not k.startswith(("IMAP_", "SMTP_"))}
        env.update(
            DATABASE_URL=f"sqlite:{self.db_path}",
            TRIPTYCH_SOCKET_PATH=str(self.sock_path),
            TRIPTYCH_LOG_PATH=str(self.dir / "t.log"),
            TRIPTYCH_EMAIL_ENABLED="false",
            TERM="xterm-256color",
        )
        for k, v in (extra or {}).items():
            if k in PROTECTED_ENV:
                raise SystemExit(f"refusing to override {k}: sandbox isolation")
            env[k] = v
        return env

    def cli(self, *args: str, env: dict[str, str] | None = None, timeout: float = 60) -> CliResult:
        p = subprocess.run([str(BIN), *args], cwd=self.dir, env=self.env(env), capture_output=True, text=True, timeout=timeout)
        return CliResult(p.returncode, p.stdout, p.stderr)

    def migrate(self) -> None:
        if not self.db_path.exists():
            self.cli("list")

    def db(self, sql: str, params: tuple = ()) -> list[tuple]:
        self.migrate()
        con = sqlite3.connect(self.db_path, timeout=10)
        try:
            rows = con.execute(sql, params).fetchall()
            con.commit()
            return rows
        finally:
            con.close()

    def db_script(self, sql: str) -> None:
        self.migrate()
        con = sqlite3.connect(self.db_path, timeout=10)
        try:
            con.executescript(sql)
        finally:
            con.close()

    def start_daemon(self, timeout: float = 30) -> subprocess.Popen:
        proc = subprocess.Popen([str(BIN), "daemon"], cwd=self.dir, env=self.env(), stdout=open(self.dir / "daemon.out", "a"),
                                stderr=subprocess.STDOUT, start_new_session=True)
        self.bg.append(proc)
        end = time.time() + timeout
        while time.time() < end:
            if proc.poll() is not None:
                break
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(str(self.sock_path))
                s.close()
                return proc
            except OSError:
                time.sleep(0.1)
        raise RuntimeError(f"daemon did not come up: {(self.dir / 'daemon.out').read_text()[-400:]}")

    def stop_bg(self) -> None:
        for p in self.bg:
            if p.poll() is None:
                p.kill()
                p.wait()
        self.bg.clear()

    def cleanup(self) -> None:
        self.stop_bg()
        shutil.rmtree(self.dir, ignore_errors=True)


class Term:
    """A program on a pty plus a pyte terminal emulator holding the current screen."""

    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], rows: int = 42, cols: int = 130):
        self.argv, self.cwd, self.env, self.rows, self.cols = argv, cwd, env, rows, cols
        self.pid: int | None = None
        self.fd: int | None = None
        self.exit_code: int | None = None
        self.eof = False
        self.last_output = time.monotonic()
        self._tail = b""
        self.screen = pyte.Screen(cols, rows)
        self.stream = pyte.ByteStream(self.screen)

    def spawn(self) -> None:
        self.screen = pyte.Screen(self.cols, self.rows)
        self.stream = pyte.ByteStream(self.screen)
        self.exit_code, self.eof, self._tail = None, False, b""
        pid, fd = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", self.rows, self.cols, 0, 0))
                os.chdir(self.cwd)
                os.execve(self.argv[0], self.argv, self.env)
            finally:
                os._exit(127)
        self.pid, self.fd = pid, fd
        self.last_output = time.monotonic()

    def _feed(self, data: bytes) -> None:
        # pyte has no alt-screen support: drop everything printed before the TUI takes over
        # (migration/banner text) by resetting when `?1049h` arrives, even if split across reads.
        buf = self._tail + data
        while (i := buf.find(ALT_ON)) != -1:
            self.stream.feed(buf[:i])
            self.screen.reset()
            buf = buf[i + len(ALT_ON):]
        keep = next((k for k in range(min(len(ALT_ON) - 1, len(buf)), 0, -1) if ALT_ON.startswith(buf[-k:])), 0)
        self.stream.feed(buf[: len(buf) - keep])
        self._tail = buf[len(buf) - keep:]

    def _reap(self) -> None:
        if self.exit_code is not None or self.pid is None:
            return
        try:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError:
            self.exit_code = -1
            return
        if pid:
            self.exit_code = os.waitstatus_to_exitcode(status)

    def pump(self, dur: float = 0.0) -> None:
        end = time.monotonic() + dur
        while True:
            if self.eof or self.fd is None:
                if (rem := end - time.monotonic()) > 0:
                    time.sleep(rem)
                return
            r, _, _ = select.select([self.fd], [], [], max(0.0, min(0.05, end - time.monotonic())))
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    data = b""
                if data:
                    self.last_output = time.monotonic()
                    self._feed(data)
                else:
                    self.eof = True
                    for _ in range(40):
                        self._reap()
                        if self.exit_code is not None:
                            break
                        time.sleep(0.025)
            if time.monotonic() >= end:
                return

    def settle(self, quiet: float = 0.3, cap: float = 3.0) -> None:
        """Pump until the program has printed nothing for `quiet` seconds (or `cap` elapses)."""
        end = time.monotonic() + cap
        while time.monotonic() < end and not self.eof:
            self.pump(0.05)
            if time.monotonic() - self.last_output >= quiet:
                return

    def alive(self) -> bool:
        self.pump(0)
        self._reap()
        return self.exit_code is None

    def wait_exit(self, timeout: float = 5.0) -> int | None:
        end = time.monotonic() + timeout
        while time.monotonic() < end and self.alive():
            self.pump(0.05)
        return self.exit_code

    def send(self, keys: list[str], delay: float = 0.05) -> None:
        for k in keys:
            try:
                os.write(self.fd, k.encode())
            except OSError:
                break
            self.pump(delay)

    def press(self, *tokens: str, delay: float = 0.05, settle: float = 0.3) -> None:
        self.send(parse_keys(list(tokens)), delay)
        self.settle(settle)

    def type(self, text: str, settle: float = 0.2) -> None:
        self.send(list(text), 0.02)
        self.settle(settle)

    def resize(self, rows: int, cols: int) -> None:
        self.rows, self.cols = rows, cols
        self.screen.resize(rows, cols)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.settle(0.4)

    def kill(self) -> None:
        if self.pid is not None and self.exit_code is None:
            try:
                os.kill(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            for _ in range(40):
                self._reap()
                if self.exit_code is not None:
                    break
                time.sleep(0.025)
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None

    def lines(self) -> list[str]:
        return [l.rstrip() for l in self.screen.display]

    def text(self) -> str:
        return "\n".join(self.lines())

    def has(self, needle: str) -> bool:
        return needle in self.text()

    def wait_for(self, needle: str, timeout: float = 5.0, gone: bool = False, regex: bool = False) -> bool:
        end = time.monotonic() + timeout
        while True:
            self.pump(0.05)
            txt = self.text()
            found = bool(re.search(needle, txt)) if regex else needle in txt
            if found != gone:
                return True
            if time.monotonic() >= end or (self.eof and not gone):
                return False

    def cursor(self) -> dict | None:
        """Calendar selection: the cell painted with CURSOR_BG. Returns day/time label/text."""
        hdr_row = next((y for y, l in enumerate(self.screen.display) if re.search(r"Mon \d\d/\d\d.*Tue", l)), None)
        if hdr_row is None:
            return None
        cols = [(m.start(), m.group(0)[:3]) for m in re.finditer(r"(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \d\d/\d\d", self.screen.display[hdr_row])]
        for y in range(self.rows):
            xs = [x for x in range(self.cols) if self.screen.buffer[y][x].bg == CURSOR_BG]
            if not xs:
                continue
            day = next((dn for cx, dn in reversed(cols) if cx <= xs[0] + 1), "?")
            label = ""
            for yy in range(y, hdr_row, -1):
                if m := re.match(r"\s*.?(\d\d[ap]m)", self.screen.display[yy]):
                    label = m.group(1)
                    break
            return {"day": day, "time": label, "row": y, "text": self.screen.display[y][xs[0]: xs[-1] + 1].strip()}
        return None

    def week_dates(self) -> list[str]:
        """Header dates like ['09/14', ..., '09/20'] (Mon..Sun), [] when not on the calendar view."""
        for l in self.screen.display:
            if re.search(r"Mon \d\d/\d\d.*Tue", l):
                return re.findall(r"(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun) (\d\d/\d\d)", l)
        return []

    def style_of(self, needle: str) -> dict | None:
        """Foreground colour and bold flag of the first cell of `needle` on screen, None if absent."""
        for y, line in enumerate(self.screen.display):
            x = line.find(needle)
            if x >= 0:
                cell = self.screen.buffer[y][x]
                return {"fg": cell.fg, "bold": cell.bold}
        return None

    def spans(self) -> list[dict]:
        """Runs of cells with a non-default background or reverse video (selection/highlight debugging)."""
        out = []
        for y in range(self.rows):
            x = 0
            while x < self.cols:
                c = self.screen.buffer[y][x]
                key = (c.bg, c.reverse)
                if c.bg != "default" or c.reverse:
                    x0 = x
                    while x < self.cols and (self.screen.buffer[y][x].bg, self.screen.buffer[y][x].reverse) == key:
                        x += 1
                    out.append({"row": y, "cols": [x0, x - 1], "bg": c.bg, "reverse": c.reverse,
                                "text": self.screen.display[y][x0:x].strip()})
                else:
                    x += 1
        return out


def render(lines: list[str], compact: bool = False) -> str:
    out = list(lines)
    while out and not out[-1]:
        out.pop()
    if compact:
        strip = str.maketrans("", "", "│┌┐└┘─")
        out = [re.sub(r" {2,}", "  ", l.translate(strip)).strip() for l in out]
        out = [l for l in out if l]
    return "\n".join(out)


# ---- session server / client -------------------------------------------------------------------

def request(name: str, payload: dict, timeout: float = 30, quiet: bool = False) -> dict | None:
    path = HOME / name / "ctl.sock"
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    try:
        s.connect(str(path))
        s.sendall((json.dumps(payload) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        return json.loads(buf)
    except (OSError, json.JSONDecodeError):
        if quiet:
            return None
        raise SystemExit(f"no live session {name!r} (start one: tuidrive -s {name} start)")
    finally:
        s.close()


def snapshot(term: Term, ok: bool = True, marks: bool = False) -> dict:
    snap = {"ok": ok, "alive": term.alive(), "exit_code": term.exit_code, "rows": term.rows, "cols": term.cols,
            "lines": term.lines(), "cursor": term.cursor(), "dates": term.week_dates()}
    if marks:
        snap["spans"] = term.spans()
    return snap


def serve(name: str) -> None:
    sb = Sandbox(name)
    cfg = json.loads((sb.dir / "session.json").read_text())
    term = Term([str(BIN)], sb.dir, sb.env(cfg["env"]), cfg["rows"], cfg["cols"])
    term.spawn()
    (sb.dir / "server.pid").write_text(str(os.getpid()))
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(str(sb.dir / "ctl.sock"))
    srv.listen(4)
    running = True
    while running:
        term.pump(0)
        if not select.select([srv], [], [], 0.05)[0]:
            continue
        conn, _ = srv.accept()
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = conn.recv(65536)
            if not chunk:
                break
            buf += chunk
        req, ok = json.loads(buf), True
        op = req["op"]
        if op == "send":
            term.send(req["keys"], req.get("delay", 0.05))
            term.settle(req.get("settle", 0.3))
        elif op == "wait":
            ok = term.wait_for(req["text"], req.get("timeout", 5.0), req.get("gone", False), req.get("regex", False))
        elif op == "resize":
            term.resize(req["rows"], req["cols"])
        elif op == "restart":
            term.kill()
            term.spawn()
            term.settle(0.6)
        elif op == "stop":
            running = False
            term.kill()
        rep = snapshot(term, ok, req.get("marks", False))
        conn.sendall((json.dumps(rep) + "\n").encode())
        conn.close()
    srv.close()
    (sb.dir / "ctl.sock").unlink(missing_ok=True)


def show(rep: dict, name: str, compact: bool = False, marks: bool = False) -> None:
    state = "alive" if rep["alive"] else f"EXITED code={rep['exit_code']}"
    print(f"--- session {name}: {state} {rep['cols']}x{rep['rows']} ---")
    print(render(rep["lines"], compact))
    if rep.get("cursor"):
        c = rep["cursor"]
        print(f"CURSOR: {c['day']} {c['time']} {c['text']!r}")
    if rep.get("dates"):
        print(f"WEEK: {rep['dates'][0]}..{rep['dates'][-1]}")
    for sp in rep.get("spans", []) if marks else []:
        print(f"SPAN r{sp['row']} c{sp['cols'][0]}-{sp['cols'][1]} bg={sp['bg']} rev={sp['reverse']} {sp['text']!r}")


def cmd_start(a: argparse.Namespace) -> int:
    if not BIN.exists() or a.build:
        subprocess.check_call(["cargo", "build"], cwd=ROOT)
    rows, cols = map(int, a.size.lower().split("x")[::-1]) if "x" in a.size else (42, 130)
    sb = Sandbox.create(a.session)
    extra = dict(kv.split("=", 1) for kv in a.env)
    sb.env(extra)  # validates protected keys
    sb.migrate()
    for pre in a.pre:
        r = sb.cli(*shlex.split(pre), env=extra)
        print(f"pre: {pre!r} rc={r.rc} {r.clean_out.strip()}")
    if a.seed:
        sb.db_script(Path(a.seed).read_text())
    (sb.dir / "session.json").write_text(json.dumps({"rows": rows, "cols": cols, "env": extra}))
    log = open(sb.dir / "server.log", "w")
    subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "-s", a.session, "_serve"], stdout=log, stderr=log,
                     start_new_session=True, env=os.environ)
    for _ in range(100):
        if (sb.dir / "ctl.sock").exists() and request(a.session, {"op": "status"}, quiet=True):
            break
        time.sleep(0.1)
    else:
        raise SystemExit(f"server failed to start: {(sb.dir / 'server.log').read_text()[-500:]}")
    print(f"started session {a.session} sandbox={sb.dir} db={sb.db_path}")
    rep = request(a.session, {"op": "send", "keys": [], "settle": 0.8})
    show(rep, a.session, a.compact)
    return 0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="tuidrive", description=__doc__.split("\n")[0])
    p.add_argument("-s", "--session", default=os.environ.get("TUIDRIVE_SESSION", "default"))
    sub = p.add_subparsers(dest="cmd", required=True)

    st = sub.add_parser("start", help="sandbox + detached TUI session")
    st.add_argument("--size", default="130x42", help="COLSxROWS (default 130x42)")
    st.add_argument("--seed", help="SQL file applied to the fresh DB before the TUI launches")
    st.add_argument("--pre", action="append", default=[], help="triptych CLI args to run first, e.g. --pre 'add \"x !!\"'")
    st.add_argument("--env", action="append", default=[], help="extra KEY=VALUE for the TUI (not DB/socket paths)")
    st.add_argument("--build", action="store_true", help="cargo build first")
    st.add_argument("--compact", action="store_true")

    for name in ("send", "type", "screen", "wait"):
        sp = sub.add_parser(name)
        sp.add_argument("--compact", action="store_true", help="strip borders, collapse spaces, drop blank rows")
        sp.add_argument("--marks", action="store_true", help="also list highlighted (bg/reverse) spans")
        if name in ("send", "type"):
            sp.add_argument("--settle", type=float, default=0.3, help="quiet seconds before returning the screen")
            sp.add_argument("--delay", type=float, default=0.05, help="pause after each key")
            sp.add_argument("-q", "--quiet", action="store_true", help="print status line only")
        if name == "send":
            sp.add_argument("keys", nargs="+", help="NAME|C-x|t:text|single char, e.g. j j ENTER t:hello")
        if name == "type":
            sp.add_argument("text")
        if name == "wait":
            sp.add_argument("text")
            sp.add_argument("--gone", action="store_true")
            sp.add_argument("--regex", action="store_true")
            sp.add_argument("--timeout", type=float, default=5.0)

    d = sub.add_parser("db", help="run SQL against the session DB (read or write)")
    d.add_argument("sql")
    c = sub.add_parser("cli", help="run the triptych CLI in the session sandbox")
    c.add_argument("--bg", action="store_true", help="run detached (for `daemon`)")
    c.add_argument("--raw", action="store_true", help="keep migration/NLP banner noise")
    c.add_argument("args", nargs=argparse.REMAINDER)
    rs = sub.add_parser("resize")
    rs.add_argument("rows", type=int)
    rs.add_argument("cols", type=int)
    rt = sub.add_parser("restart", help="kill + relaunch the TUI on the same DB")
    for sp in (rs, rt):
        sp.add_argument("--compact", action="store_true")
    sp_stop = sub.add_parser("stop")
    sp_stop.add_argument("--keep", action="store_true", help="keep the sandbox dir for inspection")
    sp_stop.add_argument("--all", action="store_true")
    sub.add_parser("ls")
    sub.add_parser("path", help="print the sandbox dir")
    sub.add_parser("_serve")

    a = p.parse_args(argv)
    name = a.session
    if a.cmd == "_serve":
        serve(name)
        return 0
    if a.cmd == "start":
        return cmd_start(a)
    if a.cmd == "ls":
        for d_ in sorted(HOME.glob("*/ctl.sock")):
            rep = request(d_.parent.name, {"op": "status"}, quiet=True)
            print(f"{d_.parent.name}\t{'alive' if rep and rep['alive'] else 'tui-exited' if rep else 'dead'}")
        return 0
    if a.cmd == "stop":
        names = [x.parent.name for x in HOME.glob("*/ctl.sock")] if a.all else [name]
        for n in names:
            request(n, {"op": "stop"}, quiet=True)
            sb = Sandbox(n)
            for _ in range(50):
                if not (sb.dir / "ctl.sock").exists():
                    break
                time.sleep(0.1)
            for pf in sb.dir.glob("bg-*.pid"):
                try:
                    os.kill(int(pf.read_text()), signal.SIGKILL)
                except (ProcessLookupError, ValueError):
                    pass
            if not a.keep:
                shutil.rmtree(sb.dir, ignore_errors=True)
            print(f"stopped {n}" + (f" (kept {sb.dir})" if a.keep else ""))
        return 0

    sb = Sandbox(name)
    if not sb.dir.exists():
        raise SystemExit(f"no session {name!r} (tuidrive -s {name} start)")
    if a.cmd == "path":
        print(sb.dir)
        return 0
    if a.cmd == "db":
        for row in sb.db(a.sql):
            print("|".join("" if v is None else str(v) for v in row))
        return 0
    if a.cmd == "cli":
        args = [x for x in a.args if x != "--"]
        if a.bg:
            proc = subprocess.Popen([str(BIN), *args], cwd=sb.dir, env=sb.env(), stdout=open(sb.dir / "bg.out", "a"),
                                    stderr=subprocess.STDOUT, start_new_session=True)
            (sb.dir / f"bg-{proc.pid}.pid").write_text(str(proc.pid))
            time.sleep(1.0)
            print(f"started pid={proc.pid} (log: {sb.dir / 'bg.out'})")
            return 0
        r = sb.cli(*args, timeout=120)
        print(f"rc={r.rc}")
        print((r.out if a.raw else r.clean_out).rstrip())
        err = (r.err if a.raw else r.clean_err).rstrip()
        if err:
            print(f"[stderr]\n{err}")
        return 0

    payload: dict = {"marks": getattr(a, "marks", False)}
    if a.cmd == "send":
        payload.update(op="send", keys=parse_keys(a.keys), settle=a.settle, delay=a.delay)
    elif a.cmd == "type":
        payload.update(op="send", keys=list(a.text), settle=a.settle, delay=0.02)
    elif a.cmd == "wait":
        payload.update(op="wait", text=a.text, gone=a.gone, regex=a.regex, timeout=a.timeout)
    elif a.cmd == "resize":
        payload.update(op="resize", rows=a.rows, cols=a.cols)
    elif a.cmd == "restart":
        payload.update(op="restart")
    else:
        payload.update(op="status")
    rep = request(name, payload, timeout=payload.get("timeout", 10) + 60)
    if a.cmd in ("send", "type") and a.quiet:
        print(f"--- session {name}: {'alive' if rep['alive'] else 'EXITED code=' + str(rep['exit_code'])} ---")
    else:
        show(rep, name, getattr(a, "compact", False), payload["marks"])
    if a.cmd == "wait" and not rep["ok"]:
        print(f"TIMEOUT waiting for {a.text!r}{' to disappear' if a.gone else ''}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
