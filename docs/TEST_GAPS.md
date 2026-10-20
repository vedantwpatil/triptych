# Test gaps

What the tests prove and what they do not, measured 2026-10-20 with `cargo-llvm-cov` 0.9.1 and
`cargo-mutants` 27.1.0. Test layout: [`../tests/CLAUDE.md`](../tests/CLAUDE.md). Pty driver and suite:
[`TUI_DRIVER.md`](./TUI_DRIVER.md). History and known issues: [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Coverage

| Run | Lines | Functions |
| --- | --- | --- |
| `cargo test` alone (287 tests when measured, 350 now; not re-measured) | 51% (5130 of 9978) | 61% (693 of 1135) |
| `cargo test` + pty suite (179 scenarios) | 87% (8709 of 9997) | 90% (1022 of 1140) |

Merged files under 80%: `bin/import_schedule.rs` 0% (one-off tool), `sync/calendar.rs` 0% (stub),
`notify/agent.rs` 44% (launchd install), `sync/config.rs` 57%, `nlp/parser.rs` 61%,
`nlp/ollama_client.rs` 76%, `email/client.rs` 79%, `cli/commands.rs` 79%, `tui/keys.rs` 80%.

112 functions run in no test. By file: `app/mail.rs` 37, then `tui/ui/email.rs`, `nlp/ollama_client.rs`,
`email/message.rs`, `email/client.rs` 6 each, `notify/agent.rs`, `email/config.rs`, `canvas.rs`,
`app/placement.rs` 4 each. List: `jq` over the merged JSON, grouping functions by file and first region line,
keeping groups whose every `count` is 0 (names are v0-mangled, so match on line numbers).

Not captured: the `triptych daemon` process and other background children are killed with SIGKILL, so
they write no profile (`cli/daemon.rs` is covered by cargo tests anyway). A line counts as covered when
some test runs it, not when a test checks its result: see the mutation numbers.

## Mutation (`cargo mutants`)

It runs the cargo tests only, not the pty suite. A survivor in code only the pty suite checks (SMTP,
daemon) is not necessarily unchecked. Timeouts count as detected (the changed code hung a test).

First pass: 694 mutants in `app/{tasks,allocation,calendar,schedule_io,repeat}.rs`, `notify.rs`,
`email/smtp.rs`, `cli/daemon.rs`, `nlp/rules.rs`, `recurrence.rs`, `urgency.rs`, `canvas.rs`. 370 caught,
186 missed, 18 timeout, 120 unviable.

Second pass: tests added in `tests/it/{tasks,allocation,calendar}_edges.rs` and in `cli`, `notify`,
`email_smtp`, `canvas`, `recurrence`, `urgency`, `nlp_rules`, then re-run with `--iterate`. 181 of the
186 are killed: 569 of 574 viable mutants detected. The 5 left cannot be killed from a cargo test:

| Mutant | Why it stays |
| --- | --- |
| `notify.rs` `send` guard `cfg!(target_os = "macos")` to `true`/`false` (2) | platform-only; cannot differ on one OS |
| `smtp.rs` `base64_encode` `\|` to `^` (2) | the OR-ed bit groups never overlap: same output |
| `smtp.rs` `send_inner` `smtp_port == 465` to `!=` | needs a trusted-cert TLS handshake; only the pty fake-SMTP scenarios reach it |

9 timeouts remain at a 120s limit: `app/tasks.rs` `submit_task` and `app/calendar.rs`
`submit_deadline_edit` to `()`, and 7 in `cli/daemon.rs`. Each leaves a test waiting forever
(`task_rx.recv()`, a daemon that never answers), so they are detected, only slowly.

Two survivors were code, not tests: `allocation.rs` `block_has_capacity` was dead (removed) and the
unreachable `_ => 0s` arm in `parse_relative_duration` (folded into `days`). `calendar.rs`
`selected_cell_date` `-` to `+` is equivalent only when today is Monday; the test kills it on other days.

Never run: `tui/keys.rs` (297 mutants), `app/mail.rs` (274), `email/*` besides `smtp.rs`, `sync/`,
`nlp/parser.rs`, `nlp/ollama_client.rs`, `app/placement.rs`. Full set is 2230 mutants.

## Determinism of the pty suite

A red scenario should be a regression, never noise. The gating suite is now repeatable (two identical
179-pass runs, one more on the instrumented binary):

- `TRIPTYCH_NOW` pins the clock for the binary (`src/clock.rs`), `fakeimap.py` and the Python checks
  (2026-06-10 18:00Z). SQL seeding must use that instant, never SQLite `'now'`.
- Every sandbox points `TRIPTYCH_OLLAMA_URL` at a closed port unless the scenario starts `fakeollama.py`,
  so the parser falls back to rules.
- Three scenarios need the user's own Ollama and are `real=True`: skipped by default, run with
  `--real-ollama` (non-gating; they unload the model). Not run when this was written.
- Left alone: email modules read the real clock (`email/smtp.rs` Date and Message-ID, `email/store.rs`,
  `email/drafts.rs`, `email/client.rs`); remaining `time.sleep` calls sit inside poll loops.

## Not done

1. Mutate the never-run modules. Do `app/mail.rs` first (37 never-run functions) if the open split
   (KI-33) goes ahead.
2. Run the `--real-ollama` tier once.

## Reproduce

```
cargo llvm-cov --no-report                      # cargo tests; builds target/llvm-cov-target/debug/triptych
D=$PWD/target/llvm-cov-target
TUIDRIVE_COVERAGE=1 TRIPTYCH_BIN=$D/debug/triptych LLVM_PROFILE_FILE="$D/suite-%p-%14m.profraw" \
  python3 tests/tui/tui_suite.py -j 4           # no %c: continuous mode breaks on macOS arm64
cargo llvm-cov report --json --output-path cov.json
cargo mutants --file src/app/tasks.rs --jobs 4 --output <dir>   # never --in-place; --iterate re-tests survivors
```

`TUIDRIVE_COVERAGE=1` makes the driver quit the TUI cleanly so counters are written. Never run the
instrumented binary by hand without `DATABASE_URL` and `TRIPTYCH_SOCKET_PATH` set: it would open the real
`todo.db`. `cargo llvm-cov show-env` + `cargo build` wrote no profile (cause not investigated); the
page-aligned-counters `RUSTFLAGS` rebuild did not finish in 10 minutes.
