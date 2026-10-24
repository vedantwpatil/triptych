#!/usr/bin/env python3
"""Apply sidecar for Triptych: fills a job application in a visible browser, then waits for approval.

The line-delimited JSON protocol is documented in src/app/apply.rs. Setup: docs/SETUP.md.

Only the fields listed in FIELDS (plus the resume upload) are ever touched, and only from the
TRIPTYCH_APPLICANT file. Work authorisation, sponsorship, demographic, consent and free-text questions
are never answered: when required they come back in `unanswered` for the user to answer in the browser.
Submitting is a separate command the app sends only after the user approves.

A digest links a Simplify job page; `entry` turns it into Simplify's redirect to the employer's own page, and
`find_form` looks for the application form in that page or in an iframe on it (a careers page embedding Greenhouse).

shortcut: Greenhouse, Lever and Ashby only, matched by field label; the selectors are unverified against
live pages, upgrade by saving a form per ATS and adding a test fixture.
"""
import json
import logging
import os
import re
import sys
import tempfile
import tomllib
from urllib.parse import urlparse

ATS_HOSTS = {"greenhouse.io": "greenhouse", "lever.co": "lever", "ashbyhq.com": "ashby"}
SIMPLIFY_JOB = re.compile(r"^https://simplify\.jobs/p/([0-9a-f-]{36})", re.I)
REQUIRED_KEYS = ("first_name", "last_name", "email", "resume")

# (name shown in the review, label regex, applicant key). Labels often carry a trailing "*".
FIELDS = [
    (name, re.compile(rf"^\s*(?:{label})\s*[*✱]?\s*$", re.I), key)
    for name, label, key in [
        ("First name", "first name", "first_name"),
        ("Last name", "last name", "last_name"),
        ("Full name", "full name|name", "full_name"),
        ("Email", "email( address)?", "email"),
        ("Phone", "phone( number)?", "phone"),
        ("LinkedIn", "linkedin( url| profile)?", "linkedin"),
        ("GitHub", "github( url| profile)?", "github"),
        ("Website", "website|portfolio( url)?|personal website", "website"),
    ]
]

CONFIRMED = re.compile(r"thank you|application (has been )?(submitted|received)", re.I)

# Visible required inputs still empty (file inputs are usually hidden, so they skip the visibility test).
OPEN_REQUIRED_JS = """() => [...document.querySelectorAll('input, textarea, select')].filter(e => {
  if (e.type === 'hidden' || e.disabled) return false;
  if (e.type !== 'file' && !(e.offsetWidth || e.offsetHeight)) return false;
  if (!(e.required || e.getAttribute('aria-required') === 'true')) return false;
  if (e.type === 'file') return e.files.length === 0;
  if (e.type === 'checkbox') return !e.checked;
  if (e.type === 'radio') return ![...document.getElementsByName(e.name)].some(r => r.checked);
  return !e.value.trim();
}).map(e => (e.type === 'radio' ? e.name : (e.labels && e.labels[0] ? e.labels[0].innerText
  : e.getAttribute('aria-label') || e.name || e.type)).trim())"""


def say(event: str, **fields) -> None:
    print(json.dumps({"event": event, **fields}), flush=True)


def detect_ats(url: str) -> str | None:
    host = urlparse(url).hostname or ""
    return next((ats for suffix, ats in ATS_HOSTS.items() if host == suffix or host.endswith("." + suffix)), None)


def entry(url: str) -> str:
    """Simplify's job page applies through a script-driven button; its /jobs/click/ address is the plain redirect to the employer."""
    job = SIMPLIFY_JOB.match(url)
    return f"https://simplify.jobs/jobs/click/{job.group(1)}" if job else url


def find_form(page, tries: int = 30):
    """The frame holding a supported ATS form, and which ATS: the page itself or an iframe on it, which loads after `load`."""
    for _ in range(tries):
        for frame in page.frames:  # the main frame comes first
            ats = detect_ats(frame.url)
            if ats:
                return frame, ats
        page.wait_for_timeout(500)
    return None, None


def load_applicant(path: str) -> dict:
    if not path:
        raise ValueError("TRIPTYCH_APPLICANT is not set")
    with open(path, "rb") as f:
        applicant = tomllib.load(f)
    missing = [k for k in REQUIRED_KEYS if not applicant.get(k)]
    if missing:
        raise ValueError(f"applicant file lacks: {', '.join(missing)}")
    if not os.path.isfile(applicant["resume"]):
        raise ValueError("applicant resume is not a file")
    applicant["full_name"] = f"{applicant['first_name']} {applicant['last_name']}"
    return applicant


def fill_form(page, applicant: dict) -> list[str]:
    done = []
    for name, label, key in FIELDS:
        box = page.get_by_label(label).first
        if applicant.get(key) and box.count() and box.is_visible():
            try:
                box.fill(str(applicant[key]))
                done.append(name)
            except Exception as e:  # a combobox or custom widget: leave it for the user
                logging.info("could not fill %s: %s", name, e)
    files = page.locator("input[type=file]")
    if files.count():
        files.first.set_input_files(applicant["resume"])  # the resume field comes first on all three
        done.append("Resume")
    return done


def review(form, ats: str, filled: list[str]) -> None:
    shot = os.path.join(tempfile.gettempdir(), "triptych-apply.png")
    form.page.screenshot(path=shot, full_page=True)
    unanswered = list(dict.fromkeys(form.evaluate(OPEN_REQUIRED_JS)))
    logging.info("review %s: filled %s, unanswered %s", ats, filled, unanswered)
    say("filled", ats=ats, filled=filled, unanswered=unanswered, screenshot=shot)


def submit(form) -> None:
    form.get_by_role("button", name=re.compile(r"^\s*submit", re.I)).first.click()
    form.get_by_text(CONFIRMED).first.wait_for(timeout=30_000)  # no confirmation: error, never "submitted"
    say("submitted")


def main() -> None:
    logging.basicConfig(filename=os.path.join(tempfile.gettempdir(), "triptych-apply.log"), level=logging.INFO)
    pw = browser = form = None
    ats, filled = "", []
    try:
        for line in sys.stdin:
            msg = json.loads(line)
            cmd = msg.get("cmd")
            logging.info("command %s", cmd)  # never the url or applicant: those carry personal data
            if cmd == "fill":
                applicant = load_applicant(os.environ.get("TRIPTYCH_APPLICANT", ""))
                from playwright.sync_api import sync_playwright  # lazy: the self-test runs without it
                pw = sync_playwright().start()
                browser = pw.chromium.launch(headless=False)
                page = browser.new_page()
                page.goto(entry(msg["url"]), wait_until="load")
                form, ats = find_form(page)
                if not form:
                    say("unsupported", ats=urlparse(page.url).hostname or "")
                    return
                filled = fill_form(form, applicant)
                review(form, ats, filled)
            elif cmd == "recheck":
                review(form, ats, filled)
            elif cmd == "submit":
                submit(form)
            elif cmd == "abort":
                return
    except Exception as e:  # whatever went wrong, the app needs to hear it and mark the posting failed
        logging.exception("apply failed")
        say("error", message=str(e))
    finally:
        if browser:
            browser.close()
        if pw:
            pw.stop()


if __name__ == "__main__":
    main()
