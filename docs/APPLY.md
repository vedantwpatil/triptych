# Auto-apply

Fills a job application in a visible browser from the TUI and submits only after you approve. Off until
`TRIPTYCH_APPLY_CMD` is set; without it `A` just opens the posting's link. Where the postings come from:
[`USAGE.md`](./USAGE.md#job-digests); config: [`SETUP.md`](./SETUP.md); code and protocol:
[`../src/app/apply.rs`](../src/app/apply.rs); history and open issues: [`development/jobs.md`](./development/jobs.md).

## Setup

```bash
cd tools/apply
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/playwright install chromium
```

The applicant file holds your details and stays outside the repo, e.g. `~/.config/triptych/applicant.toml`
(`first_name`, `last_name`, `email` and `resume` are required, the rest optional):

```toml
first_name = "Ada"
last_name = "Lovelace"
email = "ada@example.com"
phone = "555 0100"
linkedin = "https://linkedin.com/in/ada"
github = "https://github.com/ada"
website = "https://ada.example.com"
resume = "/Users/ada/resume.pdf"
```

Then in `.env` (the command is split on spaces, so its paths cannot contain any):

```
TRIPTYCH_APPLY_CMD=/path/to/triptych/tools/apply/.venv/bin/python /path/to/triptych/tools/apply/apply.py
TRIPTYCH_APPLICANT=/Users/ada/.config/triptych/applicant.toml
```

| Variable | Purpose | Default |
| --- | --- | --- |
| `TRIPTYCH_APPLY_CMD` | Sidecar command; enables the flow | unset (`A` opens the link) |
| `TRIPTYCH_APPLICANT` | Applicant TOML read by the sidecar | unset (the sidecar reports an error) |
| `TRIPTYCH_APPLY_SUBMIT` | `1` lets `y` submit; unset is review only | off |
| `TRIPTYCH_APPLY_MAX_PER_DAY` | Applications allowed in any rolling 24 hours | 5 |

## Flow

1. Select an `Apply:` task and press `A`. Digest links are Simplify job pages; the browser follows
   Simplify's own redirect to the employer, finds the application form (in the page or an embedded
   Greenhouse/Lever/Ashby iframe) and fills what the applicant file maps (name, email, phone, links,
   resume upload). Nothing else is touched.
2. A popup lists what was filled (`✓`) and every required field still empty (`▲`), then a `Next:` line,
   and only the keys that act right now. The screenshot path goes to `$TMPDIR/triptych.log`.
3. Answer the `▲` fields in the browser, then `r` rechecks the page. Then either click Submit in the
   browser and press `Enter` (the default, review-only flow), or press `y` to submit (only offered with
   `TRIPTYCH_APPLY_SUBMIT=1`). `n`/`Esc` cancels and closes the browser, losing anything typed there.
   Idle for 15 minutes cancels too.
4. After `Enter` or a successful `y` the posting becomes `applied` (counted by the daily cap), the task is
   ticked off and the browser closes.

Questions about work authorisation, sponsorship, demographics, consent and free text are never answered
for you: when required they stay in the open list until you fill them.

## Guardrails (enforced in Rust, not the sidecar)

- `y` does nothing unless `TRIPTYCH_APPLY_SUBMIT=1`, and is refused while a required field is open.
- At most `TRIPTYCH_APPLY_MAX_PER_DAY` applications per rolling 24 hours, checked when `A` is pressed.
- An `applied` posting is refused. Every attempt is logged to `$TMPDIR/triptych.log`.
- Text from the page is treated as untrusted: control characters are stripped and lengths capped.
- The sidecar logs to `$TMPDIR/triptych-apply.log` (commands and field names, never your details).

## Limits

- Greenhouse, Lever and Ashby only, detected from the host of the page or of an iframe on it, for up to
  15 seconds after the page loads. Anything else (Workday, ...) opens the link instead.
- Postings from digests stored before 2026-10-23 have no link at all; `A` then says so.
- The page selectors are unverified against live forms and will need upkeep as those sites change.
  `python3 tools/apply/test_apply.py` checks the matching logic, and the fill and scan code against a
  local page when Chromium is installed.
- Automated submission may breach a site's terms, and some employers screen out bot-like applications.
  Start with `TRIPTYCH_APPLY_SUBMIT` unset and check a few fills by eye.
