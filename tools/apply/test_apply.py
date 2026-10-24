#!/usr/bin/env python3
"""Self-check for apply.py: `python3 tools/apply/test_apply.py`. The browser part runs only when
Playwright and a Chromium are installed; it uses a local page, never a real site."""
import contextlib
import io
import json
import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import apply  # noqa: E402

PAGE = """<form>
  <label for=f>First name *</label><input id=f required>
  <label for=l>Last name</label><input id=l>
  <label for=n>Company name</label><input id=n>
  <label for=e>Email</label><input id=e type=email required>
  <label for=r>Resume</label><input id=r type=file required style="display:none">
  <label for=w>Why do you want to work here?</label><textarea id=w required></textarea>
  <label for=a>Are you authorized to work in the US?</label><input id=a required>
  <label for=c>I agree</label><input id=c type=checkbox required>
</form>"""


def test_pure() -> None:
    assert apply.detect_ats("https://boards.greenhouse.io/acme/jobs/1") == "greenhouse"
    assert apply.detect_ats("https://jobs.lever.co/acme/1") == "lever"
    assert apply.detect_ats("https://jobs.ashbyhq.com/acme/1") == "ashby"
    assert apply.detect_ats("https://acme.wd1.myworkdayjobs.com/x") is None
    assert apply.detect_ats("https://evilgreenhouse.io/x") is None, "suffix must sit on a dot boundary"
    assert apply.detect_ats("https://greenhouse.io.evil.com/x") is None
    assert apply.detect_ats("not a url") is None

    uuid = "95437628-8de3-4f6f-ba65-44f3b0ed0be8"
    click = f"https://simplify.jobs/jobs/click/{uuid}"
    assert apply.entry(f"https://simplify.jobs/p/{uuid}?utm_source=swelist") == click
    assert apply.entry(f"https://simplify.jobs/p/{uuid}/Software-Engineer-Intern") == click
    assert apply.entry("https://boards.greenhouse.io/acme/jobs/1") == "https://boards.greenhouse.io/acme/jobs/1"
    evil = f"https://simplify.jobs.evil.test/p/{uuid}"
    assert apply.entry(evil) == evil, "only simplify.jobs itself is rewritten"

    keys = lambda text: [k for _, pat, k in apply.FIELDS if pat.match(text)]
    assert keys("First name *") == ["first_name"]
    assert keys("Name") == ["full_name"] and keys("Full name") == ["full_name"]
    assert keys("Email address") == ["email"]
    assert keys("Company name") == [], "must not match a bare 'name' inside a longer label"
    assert keys("Are you authorized to work in the US?") == []
    assert keys("Gender") == []


def test_applicant() -> None:
    with tempfile.TemporaryDirectory() as d:
        resume = Path(d, "r.pdf")
        resume.write_bytes(b"%PDF")
        toml = Path(d, "me.toml")
        toml.write_text(f'first_name="Ada"\nlast_name="L"\nemail="a@b.c"\nresume="{resume}"\n')
        got = apply.load_applicant(str(toml))
        assert got["full_name"] == "Ada L"
        toml.write_text('first_name="Ada"\n')
        try:
            apply.load_applicant(str(toml))
        except ValueError as e:
            assert "last_name" in str(e) and "email" in str(e)
        else:
            raise AssertionError("an incomplete applicant file must be refused")
    try:
        apply.load_applicant("")
    except ValueError:
        pass
    else:
        raise AssertionError("an unset path must be refused")


def test_browser() -> None:
    try:
        from playwright.sync_api import sync_playwright
        pw = sync_playwright().start()
        browser = pw.chromium.launch()
    except Exception as e:
        print(f"browser part skipped: {e}".splitlines()[0])
        return
    try:
        with tempfile.TemporaryDirectory() as d:
            resume = Path(d, "r.pdf")
            resume.write_bytes(b"%PDF")
            page = browser.new_page()
            page.set_content(PAGE)
            applicant = {"first_name": "Ada", "last_name": "L", "full_name": "Ada L", "email": "a@b.c", "resume": str(resume)}
            done = apply.fill_form(page, applicant)
            assert done == ["First name", "Last name", "Email", "Resume"], done
            assert page.input_value("#n") == "", "an unlisted field must stay untouched"
            open_ = page.evaluate(apply.OPEN_REQUIRED_JS)
            assert sorted(open_) == ["Are you authorized to work in the US?", "I agree", "Why do you want to work here?"], open_

            # A careers page embedding the form in a Greenhouse iframe (served locally, no network).
            embed = browser.new_page()
            embed.route("https://job-boards.greenhouse.io/**", lambda r: r.fulfill(body=PAGE, content_type="text/html"))
            embed.set_content('<h1>Careers</h1><iframe src="https://job-boards.greenhouse.io/embed/job_app"></iframe>')
            form, ats = apply.find_form(embed, tries=10)
            assert ats == "greenhouse" and form is not embed.main_frame, (ats, form)
            assert apply.fill_form(form, applicant) == done
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                apply.review(form, ats, done)
            event = json.loads(out.getvalue())
            assert event["event"] == "filled" and "I agree" in event["unanswered"], event
            plain = browser.new_page()
            plain.set_content("<h1>No form here</h1>")
            assert apply.find_form(plain, tries=2) == (None, None)
    finally:
        browser.close()
        pw.stop()


if __name__ == "__main__":
    for name, test in list(globals().items()):
        if name.startswith("test_"):
            test()
            print("ok", name)
