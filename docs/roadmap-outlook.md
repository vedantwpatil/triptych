# Outlook parity plan

Goal (set 2026-09-30): the Email view should work and feel like Outlook. Triptych is keyboard-first,
so parity means capability and layout, not the ribbon or the mouse. Built slices and their design are
in [`roadmap-email.md`](./roadmap-email.md); day-to-day status in [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Already at parity

Focused/Other (Slice 17), color categories (22), flag as star (3), read/unread, rules (18, 21),
snooze (12), drafts (10), signatures (20), reply/reply-all/forward (2), archive and delete (5, 6),
folder browsing and discovery (13, 16), receiving attachments (8), meeting invites as tasks (19),
body search (9), filters by account, folder, attachment, unread, star and domain (11, 23-25),
thread count badge (4, 7), contact autofill, multi-account, push sync via IDLE (15).

## Gaps, in order

1. **Three-pane layout.** Folder pane (Inbox, Sent, Drafts, Archive, Deleted, per account, with unread
   counts), message list, reading pane. Replaces the `F` folder cycle and the `v` popup. Everything
   below builds on it. Needs Sent and Trash synced: only `INBOX` and the archive folder are today.
2. **Conversation view.** Expand and collapse a thread under one row; today it is a `[N]` badge.
3. **Sent and Deleted Items, with undo.** Delete is a permanent `EXPUNGE` (Slice 5); Outlook moves to
   Deleted Items. Add move-to-trash plus an undo for delete, archive and move.
4. **Multi-select and bulk actions.** The todo list already has visual mode (`v`/`V`); reuse it.
5. **Outlook search.** Operators `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `before:`,
   `after:`; scope current folder or all folders. Today `/` is a plain substring over subject, sender
   and body.
6. **Follow-up flags and pin.** Flag with a due date becomes a Triptych task with a deadline (this is
   where Triptych beats Outlook); pin keeps a message at the top.
7. **RSVP.** Accept, Tentative, Decline send an iCalendar `REPLY` through SMTP and write the event to
   the calendar. Today `M` only makes a local task (Slice 19).
8. **Compose.** Bcc, attach files, undo send and scheduled send (compose has To/Cc/Subject/Body only).
9. **Quick steps and sweep.** One key runs a saved chain (archive + mark read + category); clean up
   a conversation; sweep a sender.
10. **Calendar and People panels.** Today's events beside the inbox, a contact list. Calendar view
    and contact data already exist; this is wiring.

Out of scope: Exchange/Graph and OAuth2 (see deferred list in `roadmap-email.md`), out-of-office,
shared mailboxes, add-ins, rich HTML composing.

## Why this order

The layout first: folders, unread counts, Sent/Deleted and conversations all need a pane to live in, and
retrofitting them into popups twice is wasted work. Move-to-trash and undo come before bulk actions so
bulk delete is never irreversible. RSVP, Bcc and attachments are independent of the layout and can go
in any gap between the larger slices.

## Cost to watch

The popup flow (`v`, `Esc`, `R`/`A`/`F`, `s`) is covered by many of the 172 scenarios in
`tests/tui/tui_suite.py`. Changing it means updating those scenarios and the driver
([`TUI_DRIVER.md`](./TUI_DRIVER.md)) in the same slice, and keeping `docs/USAGE.md` keys current.
