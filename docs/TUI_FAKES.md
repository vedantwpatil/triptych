# TUI Fake Servers

Local stand-ins the scenario suite and `tuidrive.py` start inside a sandbox, so email sync and AI
summaries can be tested without a real account or model. Driver usage: [`TUI_DRIVER.md`](./TUI_DRIVER.md);
code: [`../tests/tui/`](../tests/tui/CLAUDE.md); features they cover: [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Email and IMAP

`tests/tui/fakeimap.py` is a threaded IMAP4rev1 server over TLS on `127.0.0.1` (random port), backed by
`mailbox.json` in the sandbox. It speaks what `src/email/client.rs` sends: LOGIN (`tester`/`secret`),
SELECT with UIDVALIDITY, `UID SEARCH ALL | UID n:*` (RFC-correct: `n:*` always returns the top UID),
`UID FETCH` of `RFC822.SIZE`, `RFC822`, `RFC822.HEADER`, `UID STORE` of `+/-FLAGS(.SILENT) (\Deleted)`,
`EXPUNGE` (emits `* n EXPUNGE` per removed message, sequence numbers shifted for earlier removals in
the same response), `UID MOVE`/`UID COPY` for archive, LOGOUT. No real LIST, but SELECT of a
non-INBOX name does work (Slice 13: folder browsing) once something has actually been moved/copied
there — it's backed by the matching entries in `mailbox.json["archived"]`, so a folder-filter test
can archive a message, resync, and see it land under its own folder; an account that never archives
anything still gets `NO [NONEXISTENT]` for any other name, same as before.
`mb.disable_move()` makes `UID MOVE` answer `BAD`, exercising the client's COPY+STORE+EXPUNGE
fallback. It makes a throwaway CA plus a
`localhost` leaf with the openssl CLI; the binary trusts it through `SSL_CERT_FILE` (rustls-native-certs),
so real CAs are not trusted and TLS verification stays on. `Sandbox.env()` adds the `IMAP_*` vars only
when the sandbox has a server, so an inherited real account can never be reached.

```
python3 tests/tui/tuidrive.py start --imap 3        # session with a server holding 3 messages
python3 tests/tui/tuidrive.py imap add "Late" -n 2  # add messages (--big: over 1 MiB, header-only fetch)
python3 tests/tui/tuidrive.py imap reset 9          # new UIDVALIDITY, renumber from 1
python3 tests/tui/tuidrive.py imap log | info       # commands the binary sent (password masked) | port, counts
```

In a scenario: `mb = c.imap()` then `mb.add(subject, count=, big=, sender=, attachment=)` (`attachment=True`
builds a multipart/mixed message with a `report-{uid}.pdf` part), `mb.reset_uids(n)`,
`mb.disable_move()`;
`c.sync(**env)` runs `email sync` (env overrides, e.g. `IMAP_PASSWORD="wrong"`);
`c.sb.imap_log()`; `c.wait_db(sql, want)` polls for background writes. A TUI started with a server
syncs at once (the 60s poller's first tick) and again on entering the Email view.

## Fake SMTP

`tests/tui/fakesmtp.py` is a threaded STARTTLS SMTP server on `127.0.0.1` (random port), reusing
`fakeimap.py`'s throwaway CA/leaf. It speaks what `src/email/smtp.rs` sends: EHLO, STARTTLS, a second
EHLO (RFC 3207), `AUTH LOGIN` (`tester@example.com`/`secret`), `MAIL FROM`/`RCPT TO`/`DATA` with
dot-stuffing, QUIT. Implicit TLS (port 465) is not implemented — the client only takes that path when
`SMTP_PORT=465` literally, and binding the real privileged port isn't worth it when STARTTLS already
covers the shared post-TLS code. Every accepted message is appended to `messages.log` (mail_from,
rcpt_to, headers, body) for assertions; a mode file can reject `AUTH LOGIN` or every `RCPT TO`, to
exercise the "Send failed: ..." UI path.

```
python3 tests/tui/tuidrive.py start --smtp                    # session with a fake SMTP server
python3 tests/tui/tuidrive.py smtp mode auth_fail              # ok | auth_fail | reject_recipient
python3 tests/tui/tuidrive.py smtp log                         # messages the binary sent
```

In a scenario: `sm = c.smtp()` before `c.tui()`, then `sm.set_mode(..)`, `sm.messages()`. Compose (`c`),
reply/reply-all/forward (`R`/`A`/`F` in the detail popup) and Ctrl-S all route through it once
`SMTP_*` env is set. When both a fake IMAP and fake SMTP server are active in one sandbox, `Sandbox.env()`
merges both CA certs into one `SSL_CERT_FILE` so neither clobbers the other's trust.

## Fake Ollama

`tests/tui/fakeollama.py` serves `/api/tags`, `/api/ps` and `/api/generate` on `127.0.0.1`. The binary
reaches it through `TRIPTYCH_OLLAMA_URL`, which `Sandbox.env()` sets only when the sandbox started one.
Summary prompts answer `Fake summary of: <subject>`; triage prompts (Focused Inbox classification,
Slice 17) answer `{"focused": true}` unless the subject or snippet contains a bulk-mail keyword
("newsletter", "receipt", "notification", "unsubscribe"), which flips it to `false` — word a
scenario's subject to control which side of the split a message lands on; an empty prompt is a
warm-up, anything else gets `{}`.

```
python3 tests/tui/tuidrive.py start --ollama        # session with a fake Ollama
python3 tests/tui/tuidrive.py ollama mode error     # ok | error: make /api/generate fail
python3 tests/tui/tuidrive.py ollama log            # requests received
```

In a scenario: `ol = c.sb.ollama()` (`down=True` for a dead port), `ol.set_mode(..)`, `ol.summary_requests()`,
`ol.triage_requests()`. Start it before `c.tui()`, and in any scenario that opens a long email: its body
gets summarized. A background triage pass runs after every mail sync (`apply_mail_sync`/`apply_folder_sync`),
so any scenario with `c.sb.ollama()` active and mail synced will see triage requests logged too —
account for that in request-count assertions rather than assuming only summary/parse traffic.
