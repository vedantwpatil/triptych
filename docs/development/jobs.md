# Job digest triage history

SWE List digest mail turned into scored postings and `Apply:` tasks. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/jobs/CLAUDE.md`](../../src/jobs/CLAUDE.md). Related: [`canvas-notify.md`](./canvas-notify.md)
(the alert and `tasks.links` it reuses), [`email-sync.md`](./email-sync.md).

## Changelog

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
- Open: the cap drops a good posting for good (its `skipped` row is a tombstone). `triptych jobs --all`
  shows it with `(over cap)`.
- Open: `scan` runs on any sync that stores mail, not only digest mail; it is cheap (a key lookup per
  line) but not free.
