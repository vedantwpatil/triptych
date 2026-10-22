# NLP and LLM history

Natural-language task parsing (regex layer, Ollama layer, cache), model warmup, latency. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/nlp/CLAUDE.md`](../../src/nlp/CLAUDE.md). Plans: [`../roadmap-llm.md`](../roadmap-llm.md).
Related: [`email-triage.md`](./email-triage.md), [`todo.md`](./todo.md).

## Changelog

- 2026-09-27 (next-weekday date bug): "math homework due next monday" said on a Sunday resolved to
  tomorrow instead of the Monday 8 days out. Root cause: `rules.rs`'s `parse_date_phrase` handed any
  `next <weekday>` match to `chrono_english::parse_date_string` with `Dialect::Us`, and that crate's
  US dialect treats an explicit "next friday" the same as bare "friday" (the very next occurrence) —
  only `Dialect::Uk` adds the extra week an explicit "next" implies. Confirmed by reading the
  installed crate source (`chrono-english-0.1.8/src/types.rs`): only `Direction::Next`'s branch
  checks the dialect flag, so `last`/`this`/bare weekdays are unaffected, and numeric dates (`12/25`)
  never reach `chrono_english` at all (`parse_numeric_date` handles those separately) — a one-word
  fix, `Dialect::Us` -> `Dialect::Uk`, with no other call site touched. New `tests/it/nlp_rules.rs`
  case `deadline_by_next_weekday_skips_the_immediate_occurrence` computes tomorrow's weekday name at
  runtime and asserts the resolved deadline lands 7 days past it, so the regression can't drift with
  the calendar. All pre-existing "next weekday" tests only asserted the resolved weekday name, never
  a date offset, so none needed changes. Full gate green: `cargo build`/`cargo clippy --all-targets`
  clean, `cargo test --test it` 228 passed (was 227), full TUI suite 168/168. See
  `src/nlp/CLAUDE.md`'s Weekdays gotcha.
- 2026-09-19 (KI-21, background add): `submit_task`/`apply_task_parse` (see KI-21). The Ollama scenarios
  share one model and used to race under `-j`: `daemon_prewarm_loads_model` and `todo_add_cold_model`
  unload it, and every parallel TUI start reloads it, which likely caused the one unexplained suite FAIL
  earlier today. The three now take a file lock (`_serialized`) and `_unload()` retries. Tests: 90
  in-process, suite 95.
- 2026-09-19 (KI-20, cold model): the TUI never waits for a model load. `NLPParser::set_wait_for_load(false)`
  (set in `tui::run`) makes `OllamaClient::parse` return `ModelNotLoaded` at once when `/api/ps` shows the
  model absent; the parser falls back to regex and warms the model in the background. The CLI and daemon
  still wait (90s). The 90s load limit had raised the worst-case TUI freeze from 15s to 90s. New scenario
  `todo_add_cold_model` (failed on the old code, passes now). KI-20 fixed at its root: the prompt example
  held the placeholder `<last day of next month>`, which the 7B copied (3/3), so the deadline never
  parsed; the example now holds a real date, and the prompt asks for local time. Tests: 88 in-process,
  suite 94. Found KI-21.
- 2026-09-19 (think off): `OllamaRequest` sends `think: false`. Without it `qwen3.6:27b` (a thinking
  model) put its output in a `thinking` field and returned an empty `response`, so every parse fell back
  to regex ("EOF while parsing"). With it: valid JSON, and `triptych add` in a sandbox saved the right
  deadline and 180 min (15.9s including the load). `qwen2.5:7b` is unaffected. Found KI-20 on the way.
- 2026-09-19 (KI-19, LLM prewarm): startup now loads the model through `NLPParser::prewarm` (both the
  TUI's `SyncDaemon` and `triptych daemon`), and a parse that has to wait for a load gets a 90s limit
  instead of 15s. New scenario `daemon_prewarm_loads_model` (unloads the model, starts the daemon, polls
  `/api/ps`; needs a running Ollama). Tests: 83 in-process, suite 93/93.
- 2026-09-19 (LLM baseline): `NLPParser::parse` now logs the winning layer and latency. Measured on
  M3 Max 36GB, Ollama 0.34.2, the real `build_prompt` (~670 prompt tokens, 50-75 output tokens), model
  load included in "first load". Warm parse: qwen2.5:7b 0.9-1.5s, 1.5b 0.33-0.48s, 0.5b 0.29-0.42s.
  Reload after a model is evicted (5m default `keep_alive`, waited it out): 7b 3.6s wall, 1.06s load.
  First-ever load: 7b 26s, 1.5b 18s, 0.5b 17s (`llama-server started in ..`, from the Ollama log),
  then 0.5-1.0s on later loads. `keep_alive: -1` only removes the reload, not the first load.
  Hit rate: 2/25 probe inputs reach the LLM (only unresolved deadline phrases); the probe corpus is
  the suite's regex-covered inputs plus 8 others (3 from todo.db), so real usage still needs the new log
  line (todo.db holds 3 real inputs). Not measured: cold-page-cache load after hours idle (needs
  `sudo purge`). Findings: KI-19.
- 2026-09-17: Fixed 2 panics in `nlp/rules.rs` found by the idioms audit below: (1) 6 sites did
  `.and_local_timezone(Local).unwrap()`, panicking on a DST transition — now route through
  `app::resolve_local_datetime`. (2) `"eom"` called `.with_month()` before `.with_day(1)`,
  panicking on e.g. Jan 31 → "Feb 31" — reordered. +2 tests.

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-21** An add whose text reached the LLM blocked the TUI event loop for the parse (~1-1.5s with a
  warm 7B, up to 15s if Ollama hung): `App::add_task` was awaited inline in `tui/keys.rs` and, for
  email conversion, in `app/mail.rs`. Now `App::submit_task` spawns the parse and closes the popup at
  once; the result arrives on `task_rx` and `apply_task_parse` inserts it (the same shape as KI-13's
  `deadline_rx`). Email conversion carries the email id through, so `finish_email_conversion` links the
  new task by id, and a second Enter before the first parse lands is skipped, not duplicated. The task
  goes below the cursor as it is when the parse lands. `add_task` (CLI, tests) still parses inline.
  `add_task_at_selected_cell` never parsed, so it was never affected. `tests/it/app.rs` (two tests),
  `todo_add_llm_nonblocking` (failed on the inline code, passes now).
- **KI-20** The LLM's `deadline` was dropped for "before the end of next month" (3/3 runs, the task saved
  with no deadline although `strategy=Ollama`). Root cause: the `build_prompt` example held the placeholder
  `<last day of next month>T23:59:59+00:00`, and `qwen2.5:7b` echoed it, so no parser could read it.
  The example now holds a real date (`build_prompt` computes it). Also: the prompt asked for UTC, but
  its examples used local hours, so "at 3pm" was stored as 15:00Z (11:00 EDT); it now asks for local time
  with no offset. `parse_timestamp` reads a naive timestamp as local, keeps an offset one, and logs a
  `warn` (field name only) when it drops one. Rerun on the real 7B: 3/3 deadlines saved (23:59:59 EDT),
  "at 3pm" stored as 19:00Z. `tests/it/nlp_llm.rs`.

- **KI-19** The LLM path could never warm up. (1) The prewarm in `src/sync/ollama.rs` and
  `src/cli/daemon.rs` called `nlp.parse` with a plain string, which the regex layer resolves in ~1ms, so
  Ollama was never touched. (2) The first-ever load of a model took 17-26s (see the 2026-09-19 baseline),
  longer than `OLLAMA_TIMEOUT_MS` (15s); the client disconnect aborts the load, so it restarted from zero
  next time. All 28 `/api/generate` calls in `~/.ollama/logs/server.log` (2026-09-17..19) were `499`
  ("client connection closed before llama-server finished loading"), none `200`. Fix: `NLPParser::prewarm`
  calls `OllamaClient::warm` (empty-prompt `/api/generate`, 90s limit) from both prewarm sites (the daemon
  runs it in a spawned task, so the socket answers at once), and `OllamaClient::parse` checks `/api/ps` and
  uses the 90s `OLLAMA_LOAD_TIMEOUT_MS` instead of 15s while the model is not resident. Checked against a
  temporary 17GB default model (`qwen3.6:27b`, since reverted): `add` waited 19.3s and Ollama logged
  `POST /api/generate 200 19.28s`, where the old client would have logged `499` at 15s. `daemon_prewarm_loads_model`.

- **KI-14** A bare weekday was not parsed: "call mom on sunday" kept "on sunday" in the title and got
  no date; "friday at 3pm" failed too. `parse_chrono_candidate` (`src/nlp/rules.rs`) now accepts full
  weekday names, resolved to the next such day (never today). Abbreviations (`sat`, `sun`) are left
  alone because they are ordinary words. `nlp_on_weekday`, `nlp_bare_weekday_time`.
- **KI-1** Fuzzy NLP cache returned the wrong item ("Call dad tomorrow" saved as "Call mom" once
  similarity passed 0.85). Removed the Jaro-Winkler layer and the `strsim` dependency; only exact
  matches hit the cache. `daemon_distinct_tasks`, `todo_fuzzy_cache`.
- **KI-2** NLP date/time/duration gaps. The regex parser treated each phrase as a full timestamp, so
  "tomorrow at 3pm" lost the time and "3pm-5pm" lost its length. `src/nlp/rules.rs` now has separate
  `Date`, `TimeOfDay` and `TimeRange` segments that `assemble` merges. Added `on`/`for` prefixes,
  `12/25`-style dates (a bare `1/2` is read as a fraction unless prefixed with `on`), and am/pm
  inheritance for ranges (`2-4pm`). Bare numbers ("pages 5-7", "look at 5 things") stay in the
  title. `extract_task_fields` (`src/app.rs`) now stores an Event's end - start as its duration.
  The ten `nlp_*` scenarios.
- **KI-11** NLP `eprintln!`s wrote into the TUI alt-screen. `src/nlp/parser.rs` now logs through
  `tracing`. `email_stray_output`.
- **KI-13** The TUI froze for up to 15s while NLP waited on Ollama during a deadline edit.
  `submit_deadline_edit` is now synchronous: it closes the popup, shows "Parsing deadline...", and
  spawns the parse, which reports back over `App::deadline_rx`. `run_app` (`src/main.rs`) selects on
  that channel and calls `apply_deadline_parse`. `cal_deadline_no_freeze`.

### Earlier fixes (not numbered)

- DST-transition panic in `nlp/rules.rs` (6 `.and_local_timezone().unwrap()` sites). Fixed
  2026-09-17.
- `"eom"` month-rollover panic in `nlp/rules.rs`. Fixed 2026-09-17.
