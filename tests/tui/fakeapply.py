#!/usr/bin/env python3
"""Fake apply sidecar for the TUI suite. Speaks the protocol documented in src/app/apply.rs.

The posting URL picks the behaviour: "unsupported" and "error" answer that event, "unanswered" leaves a
required field open until <FAKEAPPLY_LOG>.answered exists (`recheck` re-reads it), anything else fills
cleanly. Every line received is appended to $FAKEAPPLY_LOG, so a scenario can assert what the app sent
(and that a refused submit never arrived).
"""
import json
import os
import sys
from pathlib import Path

LOG = os.environ.get("FAKEAPPLY_LOG")


def say(event: dict) -> None:
    print(json.dumps(event), flush=True)


def filled(url: str) -> None:
    answered = bool(LOG) and Path(LOG + ".answered").exists()
    open_fields = ["Why do you want to work here?"] if "unanswered" in url and not answered else []
    say({"event": "filled", "ats": "greenhouse", "filled": ["First name", "Last name", "Email"],
         "unanswered": open_fields, "screenshot": "/tmp/fakeapply.png"})


def main() -> None:
    url = ""
    for line in sys.stdin:
        if LOG:
            with open(LOG, "a") as log:
                log.write(line)
        msg = json.loads(line)
        cmd = msg.get("cmd")
        if cmd == "fill":
            url = msg["url"]
            if "unsupported" in url:
                say({"event": "unsupported", "ats": "workday"})
                return
            if "error" in url:
                say({"event": "error", "message": "form not found"})
                return
            filled(url)
        elif cmd == "recheck":
            filled(url)
        elif cmd == "submit":
            say({"event": "submitted"})
        elif cmd == "abort":
            return


if __name__ == "__main__":
    main()
