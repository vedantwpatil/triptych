#!/usr/bin/env python3
"""End-to-end scenario suite for Triptych: drives the real binary (CLI, daemon, TUI) in sandboxes.

Every scenario gets a throwaway sandbox (own DB, socket, log). `known="KI-n"` marks a scenario that asserts
the *correct* behaviour of a bug documented in docs/DEVELOPMENT.md: it must fail today (XFAIL) and
turns into XPASS once the bug is fixed, at which point the marker should be removed.

    python3 tests/tui/tui_suite.py [--list] [--only 'cal_*,todo_add'] [-j N] [--build] [--strict] [-v]
"""
from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
import time
import traceback
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tuidrive as td  # noqa: E402  (bootstraps pyte before anything else)

SCENARIOS: dict[str, dict] = {}
DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]


def scenario(name: str, known: str | None = None):
    assert len(name) <= 28 and name not in SCENARIOS, name

    def deco(fn):
        SCENARIOS[name] = {"fn": fn, "known": known}
        return fn
    return deco


def parse_ts(s: str | None) -> datetime | None:
    if not s:
        return None
    dt = datetime.fromisoformat(s.replace(" ", "T").replace("Z", "+00:00"))
    return (dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)).astimezone()


class Ctx:
    def __init__(self, name: str):
        self.sb = td.Sandbox.create(f"su-{name}")
        self.t: td.Term | None = None
        self.fails: list[str] = []
        self.shot = ""

    def check(self, cond, msg: str) -> bool:
        if not cond:
            self.fails.append(msg)
            if not self.shot and self.t and self.t.fd is not None:
                self.shot = td.render(self.t.lines(), compact=True)
        return bool(cond)

    def eq(self, got, want, msg: str) -> bool:
        return self.check(got == want, f"{msg}: got {got!r}, want {want!r}")

    def cli(self, *a: str, **kw) -> td.CliResult:
        return self.sb.cli(*a, **kw)

    def db(self, sql: str, params: tuple = ()) -> list[tuple]:
        return self.sb.db(sql, params)

    def tui(self, rows: int = 42, cols: int = 130, wait: str = "To-Do") -> td.Term:
        self.t = td.Term([str(td.BIN)], self.sb.dir, self.sb.env(), rows, cols)
        self.t.spawn()
        self.check(self.t.wait_for(wait, 10), f"TUI did not show {wait!r} at startup")
        self.t.settle(0.4)
        return self.t

    def see(self, text: str, timeout: float = 3.0, gone: bool = False, msg: str = "") -> bool:
        ok = self.t.wait_for(text, timeout, gone=gone)
        return self.check(ok, msg or f"screen {'still has' if gone else 'lacks'} {text!r}")

    def add(self, text: str) -> None:
        self.t.press("a")
        self.t.type(text)
        self.t.press("ENTER", settle=0.5)

    def tasks(self) -> list[dict]:
        cols = "id description completed priority scheduled_at deadline duration_minutes tags item_order".split()
        return [dict(zip(cols, r)) for r in self.db(f"select {','.join(cols)} from tasks order by item_order, id")]

    def descs(self) -> list[str]:
        return [t["description"] for t in self.tasks()]

    def blocks(self) -> list[tuple]:
        return self.db("select day_of_week,start_time,end_time,block_type,title from schedule_blocks order by day_of_week,start_time")

    def seed_blocks(self, *rows: tuple) -> None:
        for r in rows:
            self.db("insert into schedule_blocks(day_of_week,start_time,end_time,block_type,title) values (?,?,?,?,?)", r)

    def seed_emails(self, n: int = 3, **over) -> None:
        now = datetime.now(timezone.utc)
        for i in range(n):
            row = dict(uid=i + 1, message_id=f"<m{i}@x>", account="work" if i % 2 == 0 else "home", from_addr=f"p{i}@ex.com",
                       from_name=f"Person {i}", subject=f"Subject {i}", date_utc=(now - timedelta(hours=i)).strftime("%Y-%m-%dT%H:%M:%SZ"),
                       snippet=f"snippet {i}", is_read=0, body_text=f"Body line one {i}\nBody line two {i}")
            row.update(over)
            self.db(f"insert into email_messages({','.join(row)}) values ({','.join('?' * len(row))})", tuple(row.values()))

    def cal(self) -> td.Term:
        self.t.press("c")
        self.see("Weekly Calendar")
        return self.t

    def cursor(self) -> str:
        cur = self.t.cursor()
        return f"{cur['day']} {cur['time']}" if cur else "none"

    def write(self, name: str, text: str) -> Path:
        p = self.sb.dir / name
        p.write_text(text)
        return p

    def cleanup(self) -> None:
        if self.t:
            self.t.kill()
        self.sb.cleanup()


def block_toml(*blocks: tuple[str, str, str, str, str]) -> str:
    return "".join(f'[[blocks]]\nday = "{d}"\ntype = "{t}"\nstart = "{s}"\nend = "{e}"\ntitle = "{ti}"\n\n' for d, t, s, e, ti in blocks)


# ---------------------------------------------------------------- CLI

@scenario("cli_add_list")
def _(c: Ctx):
    r = c.cli("add", "buy milk #home !!")
    c.check(r.rc == 0 and "Added task" in r.out, f"add: rc={r.rc} {r.out!r}")
    out = c.cli("list").out
    c.check("[HIGH]" in out and "buy milk" in out and "#home" in out and "(ID: 1)" in out, f"list output: {out!r}")


@scenario("cli_priorities")
def _(c: Ctx):
    for txt in ("p none", "p med !", "p high !!", "p urgent !!!"):
        c.cli("add", txt)
    out = c.cli("list").out
    for tag, word in (("[MED]", "p none"), ("[MED]", "p med"), ("[HIGH]", "p high"), ("[URGENT]", "p urgent")):
        c.check(any(tag in l and word in l for l in out.splitlines()), f"{tag} missing for {word!r}: {out!r}")


@scenario("cli_low_badge")
def _(c: Ctx):
    c.cli("add", "water plants priority:low")
    out = c.cli("list").out
    c.check(any("[LOW]" in l and "water plants" in l for l in out.splitlines()), f"no [LOW] badge: {out!r}")


@scenario("cli_deadline_badge")
def _(c: Ctx):
    c.cli("add", "essay by friday")
    out = c.cli("list").out
    c.check(any("[DUE" in l and "essay" in l for l in out.splitlines()), f"no [DUE ...] badge: {out!r}")


@scenario("cli_priority_escalates")
def _(c: Ctx):
    c.cli("add", "gym priority:low in 2 hours")
    c.cli("add", "someday thing priority:low")
    c.eq([t["priority"] for t in c.tasks()], [0, 0], "stored priority must stay as typed")
    lines = c.cli("list").out.splitlines()
    c.check(any("gym" in l and "[URGENT↑]" in l for l in lines), f"due-soon task not escalated: {lines!r}")
    c.check(any("someday" in l and "[LOW]" in l for l in lines), f"undated task changed: {lines!r}")
    c.cli("done", "1")
    c.check(any("gym" in l and "[LOW]" in l for l in c.cli("list").out.splitlines()), "done task still escalated")


@scenario("cli_list_empty")
def _(c: Ctx):
    out = c.cli("list").out
    c.check("No tasks yet" in out, out)


@scenario("cli_done_rm_clear")
def _(c: Ctx):
    for t in ("one", "two", "three"):
        c.cli("add", t)
    r = c.cli("done", "1")
    c.check(r.rc == 0 and "Marked task as done" in r.out, f"done: {r.out!r}")
    c.check("✓" in c.cli("list").out, "list shows no ✓ for done task")
    r = c.cli("done", "99")
    c.check(r.rc != 0 and "not found" in r.err, f"done missing id: rc={r.rc} err={r.clean_err!r}")
    r = c.cli("clear")
    c.check("Cleared 1 completed task" in r.out and "tasks" not in r.out, f"clear: {r.out!r}")
    r = c.cli("clear")
    c.check("No completed tasks" in r.out, f"clear none: {r.out!r}")
    r = c.cli("rm", "2")
    c.check(r.rc == 0 and "Removed task with ID 2" in r.out, f"rm: {r.out!r}")
    r = c.cli("rm", "2")
    c.check(r.rc != 0 and "not found" in r.err, f"rm missing: rc={r.rc} {r.clean_err!r}")
    c.eq(c.descs(), ["three"], "remaining tasks")


@scenario("cli_bad_usage")
def _(c: Ctx):
    c.check(c.cli("--help").rc == 0 and "schedule" in c.cli("--help").out, "--help")
    c.check(c.cli("frobnicate").rc != 0, "unknown subcommand accepted")
    c.check(c.cli("done", "abc").rc != 0, "non-numeric id accepted")
    c.check(c.cli("add").rc != 0, "add without description accepted")


@scenario("cli_add_empty")
def _(c: Ctx):
    r = c.cli("add", "", timeout=40)
    c.check(r.rc != 0, f"empty add exited {r.rc}")
    c.eq(c.db("select count(*) from tasks")[0][0], 0, "task rows after empty add")


@scenario("cli_quiet_output")
def _(c: Ctx):
    c.cli("add", "quiet test")
    r = c.cli("list")
    c.check(not r.err.strip(), f"stderr not empty on success: {r.err[:120]!r}")
    c.check("NLP parsing ready" not in c.cli("add", "another").out, "banner on stdout of add")


# ---------------------------------------------------------------- daemon

@scenario("daemon_lifecycle")
def _(c: Ctx):
    c.check("not running" in c.cli("status").out, "status before start")
    c.sb.start_daemon()
    c.check("Daemon is running" in c.cli("status").out, "status after start")
    r = c.cli("add", "via the daemon")
    c.check("via daemon" in r.out, f"add not routed through daemon: {r.out!r} {r.clean_err!r}")
    c.check(c.cli("list").out.count("via the daemon") == 1, "daemon-added task missing from list")
    r = c.cli("stop")
    c.check(r.rc == 0, f"stop rc={r.rc} {r.clean_err!r}")
    time.sleep(1.0)
    c.check("not running" in c.cli("status").out, "status after stop")
    c.check(c.cli("add", "direct again").out.count("via daemon") == 0, "add still tried the daemon")


@scenario("daemon_stale_socket")
def _(c: Ctx):
    p = c.sb.start_daemon()
    p.kill()
    p.wait()
    c.check(c.sb.sock_path.exists(), "expected leftover socket file after SIGKILL")
    c.check("not running" in c.cli("status").out, "stale socket reported as running")
    c.sb.start_daemon()
    c.check("Daemon is running" in c.cli("status").out, "daemon did not recover from stale socket")


@scenario("daemon_second_instance")
def _(c: Ctx):
    c.sb.start_daemon()
    p2 = subprocess.Popen([str(td.BIN), "daemon"], cwd=c.sb.dir, env=c.sb.env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    c.sb.bg.append(p2)
    time.sleep(3)
    c.check(p2.poll() is not None, "second daemon still running: it stole the socket instead of refusing")


@scenario("daemon_stop_no_daemon")
def _(c: Ctx):
    r = c.cli("stop")
    c.eq(len([l for l in r.clean_err.splitlines() if "not running" in l]), 1, "'not running' error lines on stop")
    c.check(c.cli("status").rc != 0, "status exits 0 when no daemon is running")


@scenario("daemon_distinct_tasks")
def _(c: Ctx):
    c.sb.start_daemon()
    c.cli("add", "Call mom tomorrow")
    c.cli("add", "Call dad tomorrow")
    d = c.descs()
    c.check(len(d) == 2 and any("dad" in x.lower() for x in d), f"daemon saved wrong descriptions: {d}")


# ---------------------------------------------------------------- NLP (one-shot CLI, so no persistent fuzzy cache)

def nlp(c: Ctx, text: str, desc: str | None = None, hour=None, minute=None, day: int | None = None, date: tuple | None = None,
        dur: int | None = None, prio: int | None = None, dl_weekday: int | None = None, in_hours: float | None = None,
        weekday: int | None = None):
    r = c.cli("add", text, timeout=45)
    c.check(r.rc == 0, f"add rc={r.rc} {r.clean_err!r}")
    rows = c.tasks()
    if not c.check(len(rows) == 1, f"expected 1 task, got {len(rows)}"):
        return
    t, now = rows[0], datetime.now().astimezone()
    s, dl = parse_ts(t["scheduled_at"]), parse_ts(t["deadline"])
    if desc is not None:
        c.eq(t["description"], desc, "description")
    if hour is not None:
        c.check(s and s.hour == hour and s.minute == (minute or 0), f"scheduled {s} want {hour:02d}:{(minute or 0):02d}")
    if day is not None:
        c.check(s and s.date() == (now + timedelta(days=day)).date(), f"scheduled date {s and s.date()} want today+{day}")
    if date is not None:
        c.check(s and (s.month, s.day) == date, f"scheduled date {s and (s.month, s.day)} want {date}")
    if weekday is not None:
        c.check(s and s.weekday() == weekday and s.date() > now.date(), f"scheduled {s} want next weekday {weekday}")
    if in_hours is not None:
        c.check(s and abs((s - now) - timedelta(hours=in_hours)) < timedelta(minutes=20), f"scheduled {s} want now+{in_hours}h")
    if dur is not None:
        c.eq(t["duration_minutes"], dur, "duration_minutes")
    if prio is not None:
        c.eq(t["priority"], prio, "priority")
    if dl_weekday is not None:
        c.check(dl and dl.weekday() == dl_weekday and (dl.hour, dl.minute) == (23, 59), f"deadline {dl} want weekday {dl_weekday} 23:59")


@scenario("nlp_eod")
def _(c: Ctx):
    nlp(c, "eod report", hour=17)


@scenario("nlp_relative_hours")
def _(c: Ctx):
    nlp(c, "pay rent in 2 hours", desc="pay rent", in_hours=2)


@scenario("nlp_deadline_by_day")
def _(c: Ctx):
    nlp(c, "submit by friday", desc="submit", dl_weekday=4)


@scenario("nlp_duration_deadline")
def _(c: Ctx):
    nlp(c, "MATH 475 homework by Friday 3h", dur=180, dl_weekday=4)


@scenario("nlp_tags_priority")
def _(c: Ctx):
    nlp(c, "write docs #work #dev !!!", desc="write docs", prio=3)
    c.check("#work" in c.cli("list").out and "#dev" in c.cli("list").out, "tags missing in list")


@scenario("nlp_tomorrow_at_3pm")
def _(c: Ctx):
    nlp(c, "Submit report tomorrow at 3pm #work !!", desc="Submit report", hour=15, day=1, prio=2)


@scenario("nlp_bare_at_time")
def _(c: Ctx):
    nlp(c, "call mom at 3pm", desc="call mom", hour=15)


@scenario("nlp_bare_time")
def _(c: Ctx):
    nlp(c, "gym 6pm", desc="gym", hour=18)


@scenario("nlp_24h_clock")
def _(c: Ctx):
    nlp(c, "dentist tomorrow at 15:30", desc="dentist", hour=15, minute=30, day=1)


@scenario("nlp_month_day_time")
def _(c: Ctx):
    nlp(c, "report on Sep 25 at 2pm", desc="report", hour=14, date=(9, 25))


@scenario("nlp_next_weekday_time")
def _(c: Ctx):
    nlp(c, "review next friday at 10am", desc="review", hour=10)


@scenario("nlp_time_range")
def _(c: Ctx):
    nlp(c, "meeting 3pm-5pm", desc="meeting", hour=15, dur=120)


@scenario("nlp_day_and_range")
def _(c: Ctx):
    nlp(c, "meeting tomorrow 3pm-5pm", hour=15, day=1, dur=120)


@scenario("nlp_numeric_date")
def _(c: Ctx):
    nlp(c, "party on 12/25", desc="party", date=(12, 25))


@scenario("nlp_for_duration")
def _(c: Ctx):
    nlp(c, "read book for 30m", desc="read book", dur=30)


@scenario("nlp_on_weekday")
def _(c: Ctx):
    nlp(c, "call mom on sunday", desc="call mom", weekday=6)


@scenario("nlp_bare_weekday_time")
def _(c: Ctx):
    nlp(c, "study group friday at 3pm", desc="study group", weekday=4, hour=15)


# ---------------------------------------------------------------- schedule CLI

WEEK = block_toml(("weekdays", "deepwork", "09:00", "11:00", "Deep Work"), ("monday_wednesday", "admin", "14:00", "15:00", "Admin"))


@scenario("sched_import_show")
def _(c: Ctx):
    f = c.write("s.toml", WEEK)
    r = c.cli("schedule", "import", str(f))
    c.check(r.rc == 0 and "Imported 7 schedule blocks" in r.out, f"import: {r.out!r} {r.clean_err!r}")
    out = c.cli("schedule", "show").out
    c.check("Monday:" in out and "09:00 - 11:00 [deepwork] Deep Work" in out and "Wednesday:" in out and "Saturday:" not in out, f"show: {out!r}")
    c.eq(len(c.blocks()), 7, "block rows")


@scenario("sched_export_roundtrip")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", WEEK)))
    out = c.sb.dir / "out.toml"
    r = c.cli("schedule", "export", str(out))
    c.check("Exported 7 schedule blocks" in r.out and out.exists() and "[[blocks]]" in out.read_text(), f"export: {r.out!r}")
    before = c.blocks()
    c.cli("schedule", "clear")
    c.eq(len(c.blocks()), 0, "blocks after clear")
    c.cli("schedule", "import", str(out))
    c.eq(c.blocks(), before, "blocks after export→import roundtrip")


@scenario("sched_clear_flag")
def _(c: Ctx):
    f = str(c.write("s.toml", WEEK))
    c.cli("schedule", "import", f)
    r = c.cli("schedule", "import", f, "--clear")
    c.check("Cleared existing blocks" in r.out, f"no clear message: {r.out!r}")
    c.eq(len(c.blocks()), 7, "blocks after --clear reimport")
    r = c.cli("schedule", "clear")
    c.check("Cleared 7 schedule blocks" in r.out, r.out)
    c.check("No schedule blocks" in c.cli("schedule", "show").out, "show on empty schedule")


@scenario("sched_overlap_skipped")
def _(c: Ctx):
    f = c.write("s.toml", block_toml(("monday", "deepwork", "09:00", "11:00", "First"), ("monday", "admin", "10:00", "12:00", "Second")))
    r = c.cli("schedule", "import", str(f))
    both = r.out + r.err
    c.check("Skipping overlapping block 'Second' on monday" in both, f"overlap warning: {both!r}")
    c.eq([b[4] for b in c.blocks()], ["First"], "blocks kept")


@scenario("sched_bad_input")
def _(c: Ctx):
    r = c.cli("schedule", "import", str(c.sb.dir / "nope.toml"))
    c.check(r.rc != 0 and "Import failed" in r.err, f"missing file: rc={r.rc} {r.clean_err!r}")
    r = c.cli("schedule", "import", str(c.write("bad.toml", block_toml(("someday", "deepwork", "09:00", "10:00", "X")))))
    c.check(r.rc != 0 and "Invalid day name" in r.err, f"bad day: rc={r.rc} {r.clean_err!r}")
    r = c.cli("schedule", "import", str(c.write("bad2.toml", "not = [valid")))
    c.check(r.rc != 0, "garbage TOML accepted")
    c.eq(len(c.blocks()), 0, "blocks after failed imports")


@scenario("sched_end_before_start")
def _(c: Ctx):
    f = c.write("s.toml", block_toml(("monday", "deepwork", "15:00", "10:00", "Backwards")))
    r = c.cli("schedule", "import", str(f))
    c.check(not c.blocks() or r.rc != 0, f"stored backwards block {c.blocks()}")
    c.eq(len(c.blocks()), 0, "backwards block rows")


@scenario("sched_reallocate_fits")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", WEEK)))
    c.cli("add", "small job by friday 1h")
    r = c.cli("schedule", "reallocate")
    c.check("All deadline tasks fit" in r.out, f"reallocate: {r.out!r}")
    n = c.db("select count(*), sum(allocated_minutes) from task_block_allocations")[0]
    c.check(n[0] >= 1 and n[1] == 60, f"allocations (count, minutes) = {n}")


@scenario("sched_reallocate_conflict")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", block_toml(("monday", "deepwork", "09:00", "10:00", "Tiny")))))
    c.cli("add", "huge job by friday 30h")
    r = c.cli("schedule", "reallocate")
    c.check("▲" in r.out and "huge job" in r.out and "needs 1800m" in r.out, f"conflict output: {r.out!r}")


@scenario("sched_show_allocations")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", WEEK)))
    c.cli("add", "allocated job by friday 1h")
    c.cli("schedule", "reallocate")
    c.check("allocated job" in c.cli("schedule", "show").out, "schedule show omits task allocations")


# ---------------------------------------------------------------- todo TUI

@scenario("todo_startup")
def _(c: Ctx):
    t = c.tui()
    c.check(t.has("q: quit") and t.has("c: calendar") and t.has("a: add"), "key hints missing")
    c.check(not t.has("[ ]"), "task rows on empty DB")


@scenario("todo_add_edit_cancel")
def _(c: Ctx):
    t = c.tui()
    t.press("a")
    c.see("New Task")
    t.type("hello worlx")
    t.press("BS")
    t.type("d")
    c.check(t.has("hello world"), "typed text/backspace not shown in popup")
    t.press("ESC")
    c.see("New Task", gone=True, msg="popup still open after Esc")
    c.eq(c.descs(), [], "Esc created a task")
    t.press("a")
    t.press("ENTER")
    c.see("New Task", gone=True, msg="empty Enter left the popup open")
    c.eq(c.descs(), [], "empty Enter created a task")
    c.add("hello world")
    c.check(t.wait_for(r"\[ \].*hello world", 2, regex=True), "new task not in list")
    c.eq(c.descs(), ["hello world"], "db after add")


@scenario("todo_nav_toggle_delete")
def _(c: Ctx):
    for s in ("alpha", "bravo", "charlie"):
        c.cli("add", s)
    t = c.tui()
    lines = lambda: [l for l in t.lines() if "[ ]" in l or "[x]" in l or "[✓]" in l]
    sel = lambda: next((i for i, l in enumerate(lines()) if re.search(r">\s\[", l)), -1)
    c.eq(sel(), 0, "initial selection")
    t.press("j")
    t.press("j")
    t.press("j")
    c.eq(sel(), 2, "selection after 3×j (clamped)")
    t.press("k")
    c.eq(sel(), 1, "selection after k")
    t.press("ENTER")
    c.eq(len([x for x in c.tasks() if x["completed"]]), 1, "completed rows after Enter")
    t.press("ENTER")
    c.eq(len([x for x in c.tasks() if x["completed"]]), 0, "Enter did not toggle back")
    victim = c.tasks()[sel()]["description"]
    t.press("x")
    c.check(victim not in c.descs() and len(c.descs()) == 2, f"x did not delete {victim!r}: {c.descs()}")
    c.check(not t.has(victim), "deleted task still on screen")


@scenario("todo_visual_delete")
def _(c: Ctx):
    for s in ("a1", "b2", "c3", "d4", "e5"):
        c.cli("add", s)
    t = c.tui()
    before = c.descs()
    t.press("j", "v", "j", "j")
    c.see("VISUAL")
    t.press("d")
    c.see("VISUAL", gone=True, msg="visual mode still on after d")
    c.see("Deleted 3 task")
    c.eq(c.descs(), [before[0], before[4]], "rows left after deleting the 3-row selection")
    t.press("V", "D")
    c.eq(c.descs(), [before[0]], "cursor lands on the row after the range; V then D deletes it")


@scenario("todo_visual_cancel")
def _(c: Ctx):
    for s in ("a1", "b2", "c3"):
        c.cli("add", s)
    t = c.tui()
    t.press("v", "j")
    c.see("VISUAL")
    t.press("ESC")
    c.see("VISUAL", gone=True, msg="Esc did not leave visual mode")
    c.eq(len(c.descs()), 3, "Esc must not delete anything")
    t.press("v", "j", "c")
    c.see("Weekly Calendar")
    t.press("t")
    c.see("VISUAL", gone=True, msg="visual mode survived a view switch")
    t.press("d")
    c.eq(len(c.descs()), 2, "d after cancel deletes only the cursor row")


@scenario("todo_delete_email_linked")
def _(c: Ctx):
    c.seed_emails(1, subject="Pay invoice tomorrow")
    t = c.tui()
    t.press("m", "ENTER")
    c.see("Email converted to task")
    t.press("m")
    t.press("x")
    c.check(not t.has("FOREIGN KEY"), "FK error shown on delete")
    c.eq(c.descs(), [], "linked task not deleted")
    c.eq(c.db("select task_id from email_messages")[0][0], None, "email still linked to the deleted task")


@scenario("todo_insert_order")
def _(c: Ctx):
    t = c.tui()
    for s in ("A", "B", "C"):
        c.add(s)
    c.eq(c.descs(), ["C", "B", "A"], "order after three adds at top")
    t.press("j")
    c.add("D")
    c.eq(c.descs(), ["C", "B", "D", "A"], "add inserts after the selected row")


@scenario("todo_badges_render")
def _(c: Ctx):
    c.cli("add", "urgent thing #ops !!!")
    c.cli("add", "later thing in 3 hours")
    t = c.tui()
    c.check(t.has("[URGENT]") and t.has("#ops"), "priority badge/tag not rendered")
    c.check(t.has("[TODAY") or t.has("[TMR"), "scheduled-date badge not rendered")


@scenario("todo_escalation_badges")
def _(c: Ctx):
    c.cli("add", "gym priority:low in 2 hours")
    c.cli("add", "water plants priority:low")
    c.cli("add", "essay by friday")
    t = c.tui()
    c.check(t.has("[URGENT↑]"), "escalated badge not rendered")
    c.check(t.has("[LOW]"), "[LOW] badge not rendered")
    c.check(t.has("[DUE"), "deadline badge not rendered")


@scenario("todo_urgency_colors")
def _(c: Ctx):
    c.cli("add", "selected row")  # the highlighted first row overrides badge colours
    c.cli("add", "water plants priority:low")
    c.cli("add", "read a book")
    c.cli("add", "pay bill on 12/25/2099 !!")
    c.cli("add", "gym priority:low in 2 hours")
    t = c.tui()
    # Only ANSI palette slots 0-15 (the terminal theme picks the shades). pyte reports them as the
    # xterm default hex: slot 8 = 7f7f7f, 7 = e5e5e5, 1 = cd0000, 9 = ff0000.
    want = {
        "[LOW] water": ("7f7f7f", False),
        "[MED] read": ("e5e5e5", False),
        "[HIGH]": ("cd0000", False),
        "[URGENT↑]": ("ff0000", True),
    }
    for badge, (fg, bold) in want.items():
        got = t.style_of(badge)
        c.check(got == {"fg": fg, "bold": bold}, f"{badge} style {got}, want fg={fg} bold={bold}")


@scenario("todo_persist_restart")
def _(c: Ctx):
    t = c.tui()
    c.add("remember me")
    t.press("q")
    c.eq(t.wait_exit(5), 0, "exit code after q")
    t = c.tui()
    c.check(t.has("remember me"), "task lost across restart")


@scenario("todo_quit_codes")
def _(c: Ctx):
    t = c.tui()
    t.press("q")
    c.eq(t.wait_exit(5), 0, "q exit code")
    t = c.tui()
    t.press("c")
    t.press("q")
    c.eq(t.wait_exit(5), 0, "q from calendar exit code")


@scenario("todo_tui_nlp")
def _(c: Ctx):
    t = c.tui()
    c.add("pay rent in 2 hours #bills !!")
    row = c.tasks()[0]
    c.eq(row["description"], "pay rent", "description")
    c.check(row["scheduled_at"] and row["priority"] == 2, f"nlp fields not applied: {row}")
    c.check(t.has("#bills") and t.has("[URGENT↑]"), "badges not shown after TUI add (HIGH raised: due in 2h)")


@scenario("todo_fuzzy_cache")
def _(c: Ctx):
    c.tui()
    c.add("Call mom tomorrow")
    c.add("Call dad tomorrow")
    d = c.descs()
    c.check(any("dad" in x.lower() for x in d), f"second add saved as a cached earlier parse: {d}")


@scenario("todo_view_cycle")
def _(c: Ctx):
    t = c.tui()
    t.press("c")
    c.see("Weekly Calendar")
    t.press("t")
    c.see("To-Do")
    t.press("m")
    c.see("Email (")
    t.press("m")
    c.see("To-Do")
    for key, want in (("TAB", "Weekly Calendar"), ("TAB", "Email ("), ("TAB", "To-Do"), ("BTAB", "Email ("), ("BTAB", "Weekly Calendar"), ("BTAB", "To-Do")):
        t.press(key)
        c.see(want, msg=f"{key} did not land on {want!r}")
    t.press("c")
    t.press("ESC")
    c.see("To-Do", msg="Esc from calendar")


@scenario("todo_schedule_key")
def _(c: Ctx):
    c.cli("add", "schedule me")
    t = c.tui()
    t.press("s")
    c.see("Scheduled for", msg="no scheduled status")
    c.check(c.tasks()[0]["scheduled_at"] is not None, "scheduled_at not set in DB")
    t.press("s")
    c.see("already scheduled or completed")


@scenario("todo_schedule_no_slot")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", block_toml(("everyday", "meal", "07:00", "23:00", "Grazing")))))
    c.cli("add", "nowhere to go")
    t = c.tui()
    t.press("s")
    c.see("No available slot found")
    c.check(c.tasks()[0]["scheduled_at"] is None, "task scheduled despite no free slot")


@scenario("todo_small_terminals")
def _(c: Ctx):
    c.cli("add", "resize me")
    t = c.tui()
    for view in ("", "c", "TAB"):
        if view:
            t.press(view)
        for rows, cols in ((10, 40), (3, 10), (1, 1), (42, 130)):
            t.resize(rows, cols)
            c.check(t.alive(), f"crashed at {cols}x{rows}: exit={t.exit_code}")
    c.check(t.alive() and t.has("Email"), "not rendering after returning to full size")


# ---------------------------------------------------------------- calendar TUI

@scenario("cal_render")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    dates = t.week_dates()
    c.eq(len(dates), 7, "header dates")
    c.check(all(f"{h:02d}" in t.text() for h in (7, 12, 10)) and t.has("07am") and t.has("10pm"), "time labels")
    today = datetime.now().strftime("%m/%d")
    c.check(today in dates, f"today {today} not in week {dates}")
    hour = datetime.now().hour
    if 7 <= hour <= 22:
        c.check("▸" in t.text(), "now-marker missing on the current hour")


@scenario("cal_cursor_moves")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    c.eq(c.cursor(), "Mon 07am", "start cell")
    for keys, want in ((["k"], "Mon 07am"), (["h"], "Mon 07am"), (["j", "j"], "Mon 09am"), (["l"], "Tue 09am"), (["DOWN"], "Tue 10am"),
                       (["RIGHT"], "Wed 10am"), (["UP"], "Wed 09am"), (["LEFT"], "Tue 09am")):
        t.press(*keys)
        c.eq(c.cursor(), want, f"after {keys}")
    t.press(*["l"] * 8)
    c.eq(c.cursor().split()[0], "Sun", "right edge clamp")
    t.press(*["j"] * 20)
    c.eq(c.cursor().split()[1], "10pm", "bottom edge clamp")


@scenario("cal_week_nav")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    base = t.week_dates()
    t.press("L")
    nxt = t.week_dates()
    d0 = lambda s: datetime.strptime(f"2026/{s[0]}", "%Y/%m/%d")
    c.eq((d0(nxt) - d0(base)).days, 7, "L moves a week forward")
    t.press("H", "H")
    c.eq((d0(base) - d0(t.week_dates())).days, 7, "H H lands a week back")
    c.check(c.cursor() != "none", "cursor lost after week change")


@scenario("cal_block_create")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("j", "n")
    c.see("New Schedule Block")
    c.check(t.has("deepwork") and t.has("08:00") and t.has("09:00"), "form not prefilled from cursor cell")
    t.press("TAB", "TAB", "TAB")
    t.type("Writing")
    t.press("ENTER")
    c.see("New Schedule Block", gone=True)
    c.check(c.blocks() == [(0, "08:00", "09:00", "deepwork", "Writing")], f"db blocks: {c.blocks()}")
    c.see("Block created")
    c.check(t.has("[deepwork]"), "block not drawn in grid (cells show the block type)")


@scenario("cal_block_type_cycle")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("n")
    seen = [t.text()]
    t.press("j")
    c.check(t.has("deepwork_input"), "j did not cycle block type forward")
    t.press("k", "k")
    c.check(t.has("project"), "k did not wrap block type backwards")
    t.press("ESC")


@scenario("cal_block_form_rules")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("n")
    t.press("ENTER")
    c.see("New Schedule Block", timeout=0.5, msg="empty title closed the form")
    t.press("TAB")
    t.press(*["BS"] * 6)
    t.type("ab:9z")
    start = next((l for l in t.lines() if "Start:" in l), "")
    c.check("Start: :9" in start, f"start-time field kept non-digit characters: {start.strip()!r}")
    t.press("BTAB")
    c.check(t.has("New Schedule Block"), "BTAB left the form")
    t.press("ESC")
    c.see("New Schedule Block", gone=True)
    c.eq(c.blocks(), [], "Esc created a block")


@scenario("cal_block_invalid_time")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("n", "TAB")
    t.press(*["BS"] * 6)
    t.type("25:00")
    t.press("TAB", "TAB")
    t.type("Bad")
    t.press("ENTER")
    c.eq(c.blocks(), [], "block with hour 25 saved")


@scenario("cal_block_overlap")
def _(c: Ctx):
    c.seed_blocks((0, "07:00", "09:00", "deepwork", "Existing"))
    t = c.tui()
    c.cal()
    t.press("n", "TAB", "TAB", "TAB")
    t.type("Clash")
    t.press("ENTER")
    c.eq(len(c.blocks()), 1, "overlapping block saved")


@scenario("cal_block_error_visible")
def _(c: Ctx):
    c.seed_blocks((0, "07:00", "09:00", "deepwork", "Existing"))
    t = c.tui()
    c.cal()
    t.press("n", "TAB", "TAB", "TAB")
    t.type("Clash")
    t.press("ENTER")
    c.check(t.wait_for("overlaps", 2), "rejected form gives no visible reason")


@scenario("cal_block_backwards")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("n", "TAB")
    t.press(*["BS"] * 6)
    t.type("15:00")
    t.press("TAB")
    t.press(*["BS"] * 6)
    t.type("10:00")
    t.press("TAB")
    t.type("Backwards")
    t.press("ENTER")
    c.eq(c.blocks(), [], "end-before-start block saved")


@scenario("cal_block_delete")
def _(c: Ctx):
    c.seed_blocks((0, "07:00", "08:00", "deepwork", "Focus"))
    t = c.tui()
    c.cal()
    c.check(t.has("[deepwork]"), "seeded block not drawn")
    t.press("d")
    c.see("Deleted block: Focus")
    c.eq(c.blocks(), [], "blocks after d")
    t.press("d")
    c.see("No block at this time")


@scenario("cal_blocks_from_import")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", block_toml(("tuesday", "class", "10:00", "12:00", "Lecture")))))
    t = c.tui()
    c.cal()
    t.press("l", "j", "j", "j")
    c.eq(c.cursor(), "Tue 10am", "moved onto imported block")
    c.check(t.has("[class]"), "imported block not drawn")


@scenario("cal_task_input")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("l", "j", "j", "a")
    c.see("Add Task at Tue")
    c.check(t.has("09:00am"), "popup does not show the cell time")
    t.type("write report")
    t.press("ENTER")
    row = c.tasks()[0]
    s = parse_ts(row["scheduled_at"])
    c.check(row["description"] == "write report" and s and (s.weekday(), s.hour) == (1, 9), f"task row {row}")
    c.check(t.has("write report"), "task not drawn in its cell")
    t.press("a")
    t.press("ESC")
    c.eq(len(c.tasks()), 1, "Esc in task input created a task")


@scenario("cal_task_picker")
def _(c: Ctx):
    c.cli("add", "alpha")
    c.cli("add", "bravo")
    t = c.tui()
    c.cal()
    t.press("j", "s")
    c.see("Schedule Task")
    c.check(t.has("alpha") and t.has("bravo"), "picker does not list unscheduled tasks")
    t.press("j", "ENTER")
    sched = [r for r in c.tasks() if r["scheduled_at"]]
    s = parse_ts(sched[0]["scheduled_at"]) if len(sched) == 1 else None
    c.check(s and (s.weekday(), s.hour) == (0, 8), f"picked task not scheduled at Mon 08am: {sched}")
    t.press("s")
    c.see("Schedule Task")
    t.press("ESC")
    c.see("Schedule Task", gone=True)


@scenario("cal_picker_empty")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("s", "ENTER")
    c.check(t.alive(), "crashed on picker with no tasks")
    t.press("ESC")
    c.eq(c.tasks(), [], "tasks changed")


@scenario("cal_move_task")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("a")
    t.type("movable")
    t.press("ENTER")
    t.press("m")
    c.see("Task picked up")
    t.press("l", "j")
    t.press("m")
    c.see("Task moved")
    s = parse_ts(c.tasks()[0]["scheduled_at"])
    c.check(s and (s.weekday(), s.hour) == (1, 8), f"moved to {s}, want Tue 08:00")
    t.press("m")
    c.see("Task picked up")
    t.press("ESC")
    c.check(t.has("Weekly Calendar"), "Esc while holding a task left the calendar")
    t.press("ESC")
    c.see("To-Do", msg="second Esc should leave the calendar")


@scenario("cal_unschedule")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("a")
    t.type("brief")
    t.press("ENTER")
    c.check(c.tasks()[0]["scheduled_at"], "setup: task not scheduled")
    t.press("u")
    c.check(c.tasks()[0]["scheduled_at"] is None, "u did not unschedule")
    c.check(c.tasks()[0]["description"] == "brief", "u deleted the task")


@scenario("cal_deadline_edit")
def _(c: Ctx):
    t = c.tui()
    c.cal()
    t.press("e")
    c.see("No task here to set a deadline for")
    t.press("a")
    t.type("due soon")
    t.press("ENTER")
    t.press("e")
    c.see("New deadline")
    t.type("tomorrow")
    t.press("ENTER")
    c.see("Deadline updated")
    dl = parse_ts(c.tasks()[0]["deadline"])
    c.check(dl and dl.date() == (datetime.now().astimezone() + timedelta(days=1)).date(), f"deadline {dl}")


def bad_deadline(c: Ctx) -> td.Term:
    t = c.tui()
    c.cal()
    t.press("a")
    t.type("due soon")
    t.press("ENTER")
    t.press("e")
    t.type("gibberish!!")
    t.press("ENTER")
    return t


@scenario("cal_deadline_bad_input")
def _(c: Ctx):
    t = bad_deadline(c)
    c.see("parse deadline", timeout=45, msg="no 'Couldn't parse deadline' status (waited out the UI freeze)")
    c.check(c.tasks()[0]["deadline"] is None, "unparseable deadline stored")
    c.check(t.has("Weekly Calendar") and not t.has("New deadline"), "popup not closed after submit")


@scenario("cal_deadline_no_freeze")
def _(c: Ctx):
    t = bad_deadline(c)
    t.press("j")
    c.eq(c.cursor(), "Mon 08am", "cursor after j while the deadline parse is pending (UI frozen on the Ollama call)")


@scenario("cal_stack_cycle")
def _(c: Ctx):
    c.cli("add", "one")
    c.cli("add", "two")
    t = c.tui()
    c.cal()
    for _ in range(2):
        t.press("s", "ENTER")
    c.check(len([r for r in c.tasks() if r["scheduled_at"]]) == 2, "setup: two tasks in one cell")
    a = (t.cursor() or {}).get("text", "")
    t.press("]")
    b = (t.cursor() or {}).get("text", "")
    t.press("[")
    d = (t.cursor() or {}).get("text", "")
    c.check(a != b and a == d, f"stack cycle did not alternate: {a!r} {b!r} {d!r}")


@scenario("cal_deadline_alloc_render")
def _(c: Ctx):
    c.cli("schedule", "import", str(c.write("s.toml", WEEK)))
    c.cli("add", "graded job by friday 1h")
    c.cli("schedule", "reallocate")
    alloc = c.db("select block_date from task_block_allocations order by block_date")
    c.check(alloc, "setup: no allocations")
    if not alloc:
        return
    target = datetime.strptime(alloc[0][0][:10], "%Y-%m-%d").date()
    t = c.tui()
    c.cal()
    for _ in range(6):
        if any(target.strftime("%m/%d") == d for d in t.week_dates()):
            break
        t.press("L")
    c.check(t.has("graded job"), f"allocation on {target} not drawn in the grid")


@scenario("cal_small_terminal")
def _(c: Ctx):
    c.seed_blocks((0, "07:00", "08:00", "deepwork", "Focus"))
    t = c.tui(rows=20, cols=70)
    c.cal()
    c.check(t.alive() and t.has("Weekly Calendar"), "calendar at 70x20")
    t.press("n")
    c.check(t.alive(), "block form crashed at 70x20")


# ---------------------------------------------------------------- email TUI

@scenario("email_empty")
def _(c: Ctx):
    t = c.tui()
    t.press("m")
    c.see("Email (")
    c.check(t.alive() and not t.has("Subject"), "unexpected rows")
    t.press("j", "k", "v", "ENTER", "r")
    c.check(t.alive(), "crashed on keys with an empty inbox")


@scenario("email_list_render")
def _(c: Ctx):
    c.seed_emails(3)
    t = c.tui()
    t.press("m")
    c.check(t.has("(work)") and t.has("(home)") and t.has("Person 0") and t.has("Subject 2"), "rows missing")
    c.check("> " in t.text().split("Subject 0")[0].splitlines()[-1], "first row not highlighted")
    t.press("j")
    c.check("> " in t.text().split("Subject 1")[0].splitlines()[-1], "j did not move highlight")
    t.press("ESC")
    c.see("To-Do")


@scenario("email_detail_popup")
def _(c: Ctx):
    c.seed_emails(2)
    t = c.tui()
    t.press("m", "v")
    c.see("Esc/v: close")
    c.check(t.has("Body line one 0") and t.has("p0@ex.com"), "popup lacks body/header")
    t.press("ESC")
    c.see("Esc/v: close", gone=True)
    t.press("v", "v")
    c.see("Esc/v: close", gone=True, msg="v did not toggle the popup closed")


@scenario("email_detail_scroll")
def _(c: Ctx):
    c.seed_emails(1)
    t = c.tui()
    t.press("m", "v")
    t.press(*["j"] * 6)
    c.check(t.has("p0@ex.com"), "short body scrolled its own header out of view")


@scenario("email_detail_long_scroll")
def _(c: Ctx):
    c.seed_emails(1, body_text="\n".join(f"row {i}" for i in range(80)))
    t = c.tui()
    t.press("m", "v")
    t.press(*["j"] * 120)
    c.check(t.has("row 79"), "cannot scroll to the last body line")
    t.press(*["k"] * 2)
    c.check(t.has("row 77") and not t.has("row 79"), "k did not move up right after over-scrolling")


@scenario("email_mark_read")
def _(c: Ctx):
    c.seed_emails(2)
    t = c.tui()
    t.press("m", "r")
    c.eq(c.db("select is_read from email_messages order by date_utc desc")[0][0], 1, "is_read after r")
    c.eq(c.db("select sum(is_read) from email_messages")[0][0], 1, "only selected email marked")


@scenario("email_convert_task")
def _(c: Ctx):
    c.seed_emails(1, subject="Pay invoice tomorrow")
    t = c.tui()
    t.press("m", "ENTER")
    c.see("Email converted to task")
    c.check(c.descs() and "invoice" in c.descs()[0].lower(), f"task descriptions {c.descs()}")
    c.check(c.db("select task_id from email_messages")[0][0] is not None, "email not linked to task")
    c.check(t.has("[task]"), "[task] marker missing")
    t.press("m")
    c.check(t.has("invoice") or t.has("Pay"), "converted task not in todo list")


@scenario("email_stray_output")
def _(c: Ctx):
    c.seed_emails(2, subject="Pay invoice tomorrow")
    t = c.tui()
    t.press("m", "ENTER", "j", "ENTER")
    c.see("Email converted to task")
    c.check("cache hit" not in t.text() and "»" not in t.text(), "NLP eprintln! text leaked onto the TUI screen")


@scenario("email_convert_twice")
def _(c: Ctx):
    c.seed_emails(1, subject="Pay invoice tomorrow")
    t = c.tui()
    t.press("m", "ENTER")
    c.see("Email converted to task")
    t.press("ENTER")
    c.eq(len(c.tasks()), 1, "tasks after converting the same email twice")


@scenario("email_cli")
def _(c: Ctx):
    c.check("No emails yet" in c.cli("email", "list").out, "empty list message")
    c.seed_emails(2)
    out = c.cli("email", "list").out
    c.check("Subject 0" in out and "Subject 1" in out and "(work)" in out and "ID:" in out, out)
    r = c.cli("email", "sync")
    c.check(r.rc != 0 and "not configured" in r.err, f"sync without config: rc={r.rc} {r.clean_err!r}")


# ---------------------------------------------------------------- runner

def run_one(name: str) -> dict:
    spec = SCENARIOS[name]
    t0 = time.time()
    c = Ctx(name)
    try:
        spec["fn"](c)
    except Exception:  # noqa: BLE001
        c.fails.append("EXCEPTION " + traceback.format_exc(limit=3).strip().splitlines()[-1])
    finally:
        shot = c.shot
        c.cleanup()
    failed = bool(c.fails)
    status = ("XFAIL" if failed else "XPASS") if spec["known"] else ("FAIL" if failed else "PASS")
    return {"name": name, "status": status, "known": spec["known"], "fails": c.fails, "shot": shot, "secs": round(time.time() - t0, 1)}


def select(patterns: str | None) -> list[str]:
    if not patterns:
        return list(SCENARIOS)
    pats = patterns.split(",")
    return [n for n in SCENARIOS if any(fnmatch.fnmatch(n, p) or n.startswith(p) for p in pats)]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--only")
    ap.add_argument("-j", type=int, default=4, help="parallel scenarios (default 4)")
    ap.add_argument("--build", action="store_true", help="cargo build first")
    ap.add_argument("--strict", action="store_true", help="treat XPASS (a fixed known bug) as failure")
    ap.add_argument("-v", "--verbose", action="store_true", help="print failure details + last screen for every non-pass")
    ap.add_argument("--child")
    a = ap.parse_args()
    if a.child:
        print("@@" + json.dumps(run_one(a.child)))
        return 0
    names = select(a.only)
    if a.list:
        for n in names:
            print(f"{n:30} {SCENARIOS[n]['known'] or ''}")
        return 0
    if a.build or not td.BIN.exists():
        subprocess.check_call(["cargo", "build"], cwd=td.ROOT)

    def job(n: str) -> dict:
        p = subprocess.run([sys.executable, __file__, "--child", n], capture_output=True, text=True, timeout=300)
        line = next((l for l in p.stdout.splitlines() if l.startswith("@@")), None)
        if line:
            return json.loads(line[2:])
        return {"name": n, "status": "FAIL", "known": SCENARIOS[n]["known"], "fails": [f"runner crash: {p.stderr[-300:]}"], "shot": "", "secs": 0}

    results = []
    with ThreadPoolExecutor(max_workers=max(1, a.j)) as ex:
        for r in ex.map(job, names):
            results.append(r)
            k = f" ({r['known']})" if r["known"] else ""
            print(f"{r['status']:5} {r['name']:28}{k:8} {r['secs']:5.1f}s", flush=True)
            if r["status"] in ("FAIL", "XFAIL", "XPASS") and (a.verbose or r["status"] == "FAIL"):
                for f in r["fails"][:6]:
                    print(f"        - {f}")
                if a.verbose and r["shot"]:
                    print("        screen:\n" + "\n".join("          " + l for l in r["shot"].splitlines()[:25]))
    count = lambda s: sum(1 for r in results if r["status"] == s)
    print(f"\n{count('PASS')} pass, {count('FAIL')} FAIL, {count('XFAIL')} known-bug (xfail), {count('XPASS')} XPASS (bug fixed: drop its marker)")
    for r in results:
        if r["status"] == "XPASS":
            print(f"  XPASS {r['name']} now passes -> {r['known']} looks fixed")
    return 1 if count("FAIL") or (a.strict and count("XPASS")) else 0


if __name__ == "__main__":
    sys.exit(main())
