# TUI Fake Servers

Local stand-ins the scenario suite and `tuidrive.py` start inside a sandbox, so email sync and AI
summaries can be tested without a real account or model. Driver usage: [`TUI_DRIVER.md`](./TUI_DRIVER.md);
code: [`../tests/tui/`](../tests/tui/CLAUDE.md); features they cover: [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Email and IMAP

`tests/tui/fakeimap.py` is a threaded IMAP4rev1 server over TLS on `127.0.0.1` (random port), backed by
`mailbox.json` in the sandbox. It speaks what `src/email/client.rs` sends: LOGIN (`tester`/`secret`),
SELECT with UIDVALIDITY, `UID SEARCH ALL | UID n:*` (RFC-correct: `n:*` always returns the top UID),
`UID FETCH` of `RFC822.SIZE`, `RFC822`, `RFC822.HEADER`, LOGOUT. It makes a throwaway CA plus a
`localhost` leaf with the openssl CLI; the binary trusts it through `SSL_CERT_FILE` (rustls-native-certs),
so real CAs are not trusted and TLS verification stays on. `Sandbox.env()` adds the `IMAP_*` vars only
when the sandbox has a server, so an inherited real account can never be reached.

```
python3 tests/tui/tuidrive.py start --imap 3        # session with a server holding 3 messages
python3 tests/tui/tuidrive.py imap add "Late" -n 2  # add messages (--big: over 1 MiB, header-only fetch)
python3 tests/tui/tuidrive.py imap reset 9          # new UIDVALIDITY, renumber from 1
python3 tests/tui/tuidrive.py imap log | info       # commands the binary sent (password masked) | port, counts
```

In a scenario: `mb = c.imap()` then `mb.add(subject, count=, big=, sender=)`, `mb.reset_uids(n)`;
`c.sync(**env)` runs `email sync` (env overrides, e.g. `IMAP_PASSWORD="wrong"`);
`c.sb.imap_log()`; `c.wait_db(sql, want)` polls for background writes. A TUI started with a server
syncs at once (the 60s poller's first tick) and again on entering the Email view.

## Fake Ollama

`tests/tui/fakeollama.py` serves `/api/tags`, `/api/ps` and `/api/generate` on `127.0.0.1`. The binary
reaches it through `TRIPTYCH_OLLAMA_URL`, which `Sandbox.env()` sets only when the sandbox started one.
Summary prompts answer `Fake summary of: <subject>`, an empty prompt is a warm-up, others get `{}`.

```
python3 tests/tui/tuidrive.py start --ollama        # session with a fake Ollama
python3 tests/tui/tuidrive.py ollama mode error     # ok | error: make /api/generate fail
python3 tests/tui/tuidrive.py ollama log            # requests received
```

In a scenario: `ol = c.sb.ollama()` (`down=True` for a dead port), `ol.set_mode(..)`, `ol.summary_requests()`.
Start it before `c.tui()`, and in any scenario that opens a long email: its body gets summarized.
