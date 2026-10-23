# Job digest triage history

SWE List digest mail turned into scored postings and `Apply:` tasks. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/jobs/CLAUDE.md`](../../src/jobs/CLAUDE.md). Related: [`canvas-notify.md`](./canvas-notify.md)
(the alert and `tasks.links` it reuses), [`email-sync.md`](./email-sync.md).

## Changelog

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
  part) lets a posting take the `href` of the anchor whose text equals its `Company: Role` line; that
  URL becomes the task's `tasks.links`, so `o` opens it. `NewTask` gained `links`. Tests:
  `tests/it/jobs.rs`, `tests/it/cli.rs` (`jobs_turns_a_digest_...`), suite scenario `jobs_digest_to_task`.
  Checked on a copy of the real `todo.db` with `max_age_days = 3650`: 572 postings from 10 digests, 90
  shortlisted, the rest skipped.

## Known issues and open questions

- Open: the digest HTML has never been seen (the local `todo.db` holds `body_text` only, `links` is NULL
  for every stored digest), so anchor pairing is unverified against a real digest. A posting with no
  matching anchor still gets its task, with no link. Needs one saved raw digest `.eml`.
- Open: scoring reads the role only, so `Data` and `AI` hits shortlist some weak fits (business or
  governance roles); the `down` list is the knob (`TRIPTYCH_JOBS_PROFILE`).
- Open: a posting stored without a rating (Ollama down, or past the 30-rating budget) is never rated
  later; its key is already known. Backfilling a long history while Ollama is off loses the model's
  say for those postings. The real model's quality on titles alone is unmeasured: only the fake has run.
- Open: the cap drops a good posting for good (its `skipped` row is a tombstone). `triptych jobs --all`
  shows it with `(over cap)`.
- Open: `scan` runs on any sync that stores mail, not only digest mail; it is cheap (a key lookup per
  line) but not free.
