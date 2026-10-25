# Job digest triage history

SWE List digest mail turned into scored postings and `Apply:` tasks. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/jobs/CLAUDE.md`](../../src/jobs/CLAUDE.md). Related: [`canvas-notify.md`](./canvas-notify.md)
(the alert and `tasks.links` it reuses), [`email-sync.md`](./email-sync.md).

## Changelog

- 2026-10-23 (links fixed): every `Apply:` task said "No apply link on this posting". The one digest with
  stored anchors showed the real shape: the anchor text is the role alone (the company is plain text
  beside it) and the target is a Simplify page, `simplify.jobs/p/<uuid>`, so the company-plus-role match
  never hit; the 100-anchor cap also cut a 110-posting digest. `parse_digest` now walks the lines in body
  order with a cursor into the anchors and pairs each posting with the next anchor whose text is its role
  (a collapsed repeat uses up its own anchor; a role with no anchor leaves the cursor alone), `MAX_LINKS`
  is 400, and `scan` (`unseen`) fills `job_postings.url`, and an empty `tasks.links`, for postings stored
  earlier once their digest yields a link. On a copy of the real DB: 91 postings paired, and 6 of 6 sampled
  matched their Simplify page's company and role, four of them the shared "Software Engineer Intern".
  Sidecar: `entry` turns a Simplify job page (its Apply button is script-driven and only opens a login
  dialog) into the `/jobs/click/<uuid>` redirect to the employer, and `find_form` looks for the ATS form
  in the page or an iframe on it (MongoDB's careers page embeds Greenhouse). A read-only probe of that
  posting found the iframe and matched the First/Last name, Email, Phone, LinkedIn, GitHub and Website
  labels. Tests: `tests/it/jobs.rs` (role-only pairing, backfill, 150 anchors), `tools/apply/test_apply.py`.
- 2026-10-23 (apply flow): `A` on an `Apply:` task runs the sidecar named by `TRIPTYCH_APPLY_CMD` (setup:
  [`../APPLY.md`](../APPLY.md)); unset, or no `job_postings` row, it opens the link like `o`. `src/app/apply.rs`
  spawns it (`tokio::process`, `kill_on_drop`), talks line-delimited JSON (protocol in the file header) and
  reports over `apply_rx`, which `run_app` drains into `apply_apply_result`; a key handler never waits on
  it. `ApplySession` in `App.apply` routes keys to a review popup (`handle_apply_key`: `y`, `r`, `n`/`Esc`).
  Rust holds the guardrails: `y` needs `TRIPTYCH_APPLY_SUBMIT=1` and no open required field, a rolling-24h
  cap (`TRIPTYCH_APPLY_MAX_PER_DAY`, 5) counts `job_postings.applied_at`, `applied` postings are refused,
  and sidecar text is cleaned and capped in `parse_event`. Statuses added: `filling`, `filled`, `applied`
  (also ticks the task), `failed`; cancel returns to `new`. `unsupported` opens the link. Esc is refused
  while submitting, since the click may already have happened. `tools/apply/apply.py` (Playwright,
  Greenhouse/Lever/Ashby) fills only the applicant file's fields and the resume. Tests: `tests/it/apply.rs`,
  seven `apply_*` suite scenarios against `tests/tui/fakeapply.py`, `tools/apply/test_apply.py`.
- 2026-10-23 (model rating): a posting whose rule score lands in `score::LLM_BAND` (-1 to 1: no word hit,
  or up and down cancelling) and is not excluded is rated 0-10 by Ollama's main model
  (`NLPParser::job_fit`, title and company only; the prompt carries the profile's `up`/`down` words and
  fences the posting as untrusted data). `Verdict::apply_fit` adds `fit - 5` to the rule score, so 5 is
  neutral and a rating nudges rather than overrides; the reason gains `llm:8 <words>` (cleaned to one
  printable line, 80 characters). A `Rater` in `jobs.rs` holds the scan-wide state: at most 30 ratings per
  scan, and the first failure (Ollama down) turns the model off for the rest, so those postings keep their
  rule score. `sync_pass` gets the daemon's `NLPParser`; `triptych jobs` uses the app's. `ollama_client`
  gained a private `post_generate` that `summarize`, `triage` and `job_fit` share (triage now also fails
  on an HTTP error status, and waits up to 105s on a cold model instead of 90s). Tests: `tests/it/jobs.rs`,
  `tests/it/nlp_llm.rs`, suite scenarios `jobs_llm_band` and `jobs_llm_down_keeps_rules`, with a job-prompt
  branch in `fakeollama.py`.
- 2026-10-23 (rule triage): `jobs::scan` reads digests (`Profile.senders`, default
  `noreply@swelist.com`) stored in the last `max_age_days` (14), parses the lines between
  `Link to Simplify` and `See more details` (`jobs/digest.rs`; a body missing either marker yields
  nothing), scores each unseen posting by whole words in the role (`jobs/score.rs`: `up` +2, `down` -3,
  `exclude` rules it out, shortlist at 2) and stores it in `job_postings`. The best `max_per_digest` (10)
  shortlisted postings become `Apply: Company - Role` tasks (priority medium, no deadline, so the
  allocator ignores them); the rest are `skipped`. `job_postings.key` (lowercase `company|role`) is the
  dedupe and the tombstone: a posting is scored once and a deleted task is never recreated.
  `account_sync_loop`'s `sync_pass` runs the scan after each sync that stored mail, then shows one
  `notify::shortlist_alert`; digest senders are left out of the plain new-mail alert. `triptych jobs
  [--all]` runs the same scan and prints the stored postings (`--all` adds skipped, with the rule hits).
  `email_messages.links` (JSON `[{text, href}]`, `email/links.rs`, filled by `parse_raw` from the HTML
  part) lets a posting take the `href` of the anchor whose text equals its `Company: Role` line (wrong for
  real digests, replaced by role-only pairing, see above); that
  URL becomes the task's `tasks.links`, so `o` opens it. `NewTask` gained `links`. Tests:
  `tests/it/jobs.rs`, `tests/it/cli.rs` (`jobs_turns_a_digest_...`), suite scenario `jobs_digest_to_task`.
  Checked on a copy of the real `todo.db` with `max_age_days = 3650`: 572 postings from 10 digests, 90
  shortlisted, the rest skipped.

## Known issues and open questions

- Resolved 2026-10-23: the digest HTML had never been seen, so anchor pairing was unverified (and wrong:
  see "links fixed"). A posting with no matching anchor still gets its task, with no link.
- Open: digests stored before 2026-10-23 have no `links` (the HTML was never kept), so their postings and
  tasks have no page; only a re-fetch of those messages could recover them. A task the user deleted stays
  deleted (the tombstone), links or not.
- Open: scoring reads the role only, so `Data` and `AI` hits shortlist some weak fits (business or
  governance roles); the `down` list is the knob (`TRIPTYCH_JOBS_PROFILE`).
- Open: a posting stored without a rating (Ollama down, or past the 30-rating budget) is never rated
  later; its key is already known. Backfilling a long history while Ollama is off loses the model's
  say for those postings. The real model's quality on titles alone is unmeasured: only the fake has run.
- Open: the cap drops a good posting for good (its `skipped` row is a tombstone). `triptych jobs --all`
  shows it with `(over cap)`.
- Open: the Playwright adapters have filled only a local page (`test_apply.py`) and been probed read-only
  on one live Greenhouse embed (labels matched, nothing filled or uploaded). Lever and Ashby, the file
  upload and the confirmation text on a real form are unchecked until one real `A` run is reviewed.
- Open: `TRIPTYCH_APPLY_CMD` is split on whitespace (no quoted paths).
- Resolved 2026-10-25, no code change: the claim that killing the TUI or quitting mid-review (Ctrl+C;
  `q` is ignored while applying) leaves the browser open was wrong. Measured with Playwright 1.64.1 on
  macOS (headless shell and Chrome for Testing; a headed window was not tried): SIGKILL (`kill_on_drop`)
  or SIGTERM to the sidecar, SIGKILL to the TUI, and SIGHUP or SIGTERM to the whole process group
  (terminal closed) all end with no Chromium left. The Playwright node driver sees its stdin close, or
  catches the signal, and closes the browser. Chromium runs in its own process group, so group signals
  never reach it directly. Two limits: a TUI killed while the sidecar is inside a Playwright call leaves
  the browser up until that call returns (at most the 30s submit confirmation wait), and SIGKILL to the
  driver itself (`kill -9` on the group, or an out-of-memory kill) leaves Chrome for Testing running.
  Nothing in-process can catch that.
- Open: the daily cap is checked only when `A` is pressed, so it counts finished applications, not the
  one in review. One session at a time makes that safe.
- Open: `scan` runs on any sync that stores mail, not only digest mail; it is cheap (a key lookup per
  line) but not free.
