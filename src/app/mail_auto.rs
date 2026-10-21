//! Email passes that run by themselves after each sync: AI triage (Focused/Other) and saved rules.

use std::sync::Arc;

use super::{App, InputMode};
use crate::email::{EmailConfig, EmailMessage, EmailRule, store as email_store};

/// Outcome of a background triage pass over every not-yet-classified email, sent over
/// `App::triage_rx`.
#[derive(Debug, Default)]
pub struct TriageDone {
    /// `(email_id, focused)` for every email successfully classified this pass.
    results: Vec<(i64, bool)>,
    /// Last report of a drain: only this one clears `App::triage_running`.
    finished: bool,
}

/// Not-yet-classified emails per triage pass. The drain keeps running passes until none are left,
/// so this only sets how often the list is told about progress (and how long one pass holds the
/// model). Lowered from 20: most rows resolve via `bulk_mail_heuristic` with no Ollama call at all.
const TRIAGE_BATCH_LIMIT: i64 = 10;
/// Longest rest between two triage passes (see `run_email_triage`).
const TRIAGE_COOLDOWN_MAX: std::time::Duration = std::time::Duration::from_secs(10);
/// Cap on how many not-yet-checked emails one `run_email_rules` pass checks against every saved
/// rule. Unlike `TRIAGE_BATCH_LIMIT`, this is pure local-DB work (no per-item Ollama/IMAP round
/// trip), so a larger batch per pass costs nothing extra.
const RULE_CHECK_LIMIT: i64 = 100;

impl App {
    /// Runs a background triage pass (Outlook's Focused Inbox split) over every email not yet
    /// classified, capped at `TRIAGE_BATCH_LIMIT` per pass, wired in after every sync
    /// (`apply_mail_sync`, `apply_folder_sync`). Spawned and never awaited, same reasoning as
    /// `start_email_sync`; a second pass is not started while one is running (`triage_running`).
    /// `NLPParser::triage`, like `summarize`, ignores the sticky `ollama_available` flag, so this
    /// still tries and simply logs+skips per-email on failure rather than erroring the whole pass.
    pub fn run_email_triage(&mut self) {
        if self.triage_running {
            return;
        }
        self.triage_running = true;

        let (db_pool, parser, tx) = (
            self.db_pool.clone(),
            Arc::clone(&self.nlp_parser),
            self.triage_tx.clone(),
        );
        tokio::spawn(async move {
            // Drain the backlog `TRIAGE_BATCH_LIMIT` rows at a time, reporting every pass so the
            // list fills in as it goes. A failed row ends the drain (Ollama is most likely down and
            // the same row would head the next pass): the next sync retries.
            loop {
                let started = std::time::Instant::now();
                let pending = email_store::pending_triage(&db_pool, TRIAGE_BATCH_LIMIT)
                    .await
                    .unwrap_or_default();
                let mut done = TriageDone {
                    finished: pending.is_empty(),
                    ..TriageDone::default()
                };
                for email in pending {
                    let snippet = email.snippet.as_deref().unwrap_or("");
                    match parser.triage(&email.subject, snippet).await {
                        Ok(focused) => {
                            if email_store::set_triage(&db_pool, email.id, focused)
                                .await
                                .is_ok()
                            {
                                done.results.push((email.id, focused));
                            } else {
                                done.finished = true;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("[Email] triage failed for {}: {e}", email.id);
                            done.finished = true;
                        }
                    }
                }
                let finished = done.finished;
                if tx.send(done).is_err() || finished {
                    break;
                }
                // Rest as long as the pass worked, so a big backlog uses about half the model;
                // heuristic-only passes finish at once and barely pause.
                tokio::time::sleep(started.elapsed().min(TRIAGE_COOLDOWN_MAX)).await;
            }
        });
    }

    /// Applies a finished background triage pass: updates each classified email's in-memory
    /// `triage_focused` directly (rather than a full `refresh_emails` reload, which would also
    /// re-run the whole filter chain and could move the cursor) and clears the running guard.
    pub fn apply_triage(&mut self, done: TriageDone) {
        if done.finished {
            self.triage_running = false;
        }
        for (id, focused) in done.results {
            if let Some(email) = self.emails.iter_mut().find(|e| e.id == id) {
                email.triage_focused = Some(focused);
            }
        }
    }

    /// Opens the rules popup (`R` in the email list), loading every saved rule fresh from the DB.
    pub async fn open_rules_list(&mut self) {
        match email_store::list_rules(&self.db_pool).await {
            Ok(rules) => {
                self.rules = rules;
                self.selected_rule = 0;
                self.rules_open = true;
            }
            Err(e) => self.notify(&format!("Failed to load rules: {e}")),
        }
    }

    pub const fn close_rules_list(&mut self) {
        self.rules_open = false;
    }

    /// Starts typing a rule spec (`n` in the rules popup); `commit_rule_input`/`cancel_rule_input`
    /// ends it. See `parse_rule_spec` for the expected shape.
    pub fn start_rule_input(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::EmailRuleInput;
    }

    pub fn cancel_rule_input(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Ends typing and, on a valid spec, saves the new rule and reloads the popup's list. An
    /// unparseable spec or a database error reports a status message and adds nothing.
    pub async fn commit_rule_input(&mut self) {
        let typed = self.input_buffer.trim().to_string();
        self.cancel_rule_input();
        let Some((match_field, pattern, action)) = parse_rule_spec(&typed) else {
            self.notify(
                "Rule not understood (e.g. 'subject newsletter star', 'from noreply archive')",
            );
            return;
        };
        match email_store::create_rule(&self.db_pool, &match_field, &pattern, &action).await {
            Ok(_) => self.open_rules_list().await,
            Err(e) => self.notify(&format!("Failed to save rule: {e}")),
        }
    }

    /// Deletes the selected rule from the DB and the open list, clamping the selection so it stays
    /// in bounds — same shape as `delete_selected_draft`.
    pub async fn delete_selected_rule(&mut self) {
        let Some(rule) = self.rules.get(self.selected_rule) else {
            return;
        };
        let id = rule.id;

        if let Err(e) = email_store::delete_rule(&self.db_pool, id).await {
            self.notify(&format!("Failed to delete rule: {e}"));
            return;
        }

        self.rules.remove(self.selected_rule);
        if self.selected_rule >= self.rules.len() && self.selected_rule > 0 {
            self.selected_rule -= 1;
        }
    }

    /// Applies every saved rule to messages not yet checked (`rule_applied = 0`), capped at
    /// `RULE_CHECK_LIMIT` per pass — wired in after every sync (`apply_mail_sync`,
    /// `apply_folder_sync`), like `run_email_triage`. `star`/`read` are applied inline and
    /// awaited (pure local-DB work, no long-running I/O to keep off the render loop); `archive`/
    /// `delete` (Slice 21) are spawned by `apply_rule_action` itself and only reported back later
    /// over `archive_tx`/`delete_tx`, same as their manual (`a`/`d`) counterparts.
    pub async fn run_email_rules(&mut self) {
        let rules = email_store::list_rules(&self.db_pool)
            .await
            .unwrap_or_default();
        if rules.is_empty() {
            return;
        }
        // New mail first, then the backlog a rule made after it was synced (no `delete` there).
        for backlog in [false, true] {
            let pending = email_store::pending_rule_check(&self.db_pool, backlog, RULE_CHECK_LIMIT)
                .await
                .unwrap_or_default();
            for email in pending {
                for rule in rules
                    .iter()
                    .filter(|r| rule_fires(r, &email.subject, &email.from_addr, !backlog))
                {
                    self.apply_rule_action(&email, &rule.action).await;
                }
                if let Err(e) = email_store::mark_rule_checked(&self.db_pool, email.id).await {
                    tracing::warn!("[Email] rule check mark failed for {}: {e}", email.id);
                }
            }
        }
    }

    /// Runs one rule's action against one email. `star`/`read` write straight to the DB and patch
    /// the matching `App::emails` entry in memory (if loaded) — same reasoning as `apply_triage`,
    /// avoids a full `refresh_emails` reload mid-pass. `archive`/`delete` (Slice 21) instead hand
    /// off to `spawn_archive`/`spawn_delete`: a network call can't be awaited inline here without
    /// stalling every other rule and email in the same sync pass, so their local-row cleanup
    /// happens later, generically, via `apply_archive_result`/`apply_delete_result` — a rule match
    /// is indistinguishable from a manual `a`/`d` press once spawned. Silently no-ops (skips this
    /// action, still marks the email checked) when the account has no IMAP config or an invalid
    /// UID, same as the manual paths' guard clauses minus the user-facing `notify`.
    async fn apply_rule_action(&mut self, email: &EmailMessage, action: &str) {
        match action {
            "star" => {
                if email_store::set_starred(&self.db_pool, email.id, true)
                    .await
                    .is_ok()
                    && let Some(e) = self.emails.iter_mut().find(|e| e.id == email.id)
                {
                    e.is_starred = true;
                }
            }
            "read" => {
                if email_store::mark_read(&self.db_pool, email.id)
                    .await
                    .is_ok()
                    && let Some(e) = self.emails.iter_mut().find(|e| e.id == email.id)
                {
                    e.is_read = true;
                }
            }
            "archive" | "delete" => {
                let Some(config) = EmailConfig::for_account(&email.account) else {
                    return;
                };
                let Ok(uid) = u32::try_from(email.uid) else {
                    return;
                };
                let folder = email.folder.clone();
                if action == "archive" {
                    self.spawn_archive(config, folder, uid, email.id);
                } else {
                    self.spawn_delete(config, folder, uid, email.id);
                }
            }
            _ => {}
        }
    }
}

/// Tests whether `rule` matches a message's subject or from-address.
///
/// `match_field` selects which (`"from_addr"`, else `"subject"`); the test itself is a
/// case-insensitive substring, not a regex — keeps rule specs typeable in one line
/// (see `parse_rule_spec`).
#[must_use]
pub fn match_rule(rule: &EmailRule, subject: &str, from_addr: &str) -> bool {
    let haystack = if rule.match_field == "from_addr" {
        from_addr
    } else {
        subject
    };
    haystack
        .to_lowercase()
        .contains(&rule.pattern.to_lowercase())
}

/// Whether `rule` should act on a message. `fresh` is false for mail that was already synced when
/// a rule was made (`create_rule`'s backlog): the permanent `delete` action never runs there.
#[must_use]
pub fn rule_fires(rule: &EmailRule, subject: &str, from_addr: &str, fresh: bool) -> bool {
    (fresh || rule.action != "delete") && match_rule(rule, subject, from_addr)
}

/// Parses a rule spec typed in the rules popup's `n` prompt: `<field> <pattern...> <action>`.
///
/// E.g. `subject newsletter star` or `from noreply archive`. `field` is `subject` or
/// `from`/`sender` (mapped to the stored `from_addr`); `action` is `star`, `read`, `archive` or
/// `delete` (Slice 21 added the latter two — see `apply_rule_action`); `pattern` is everything in
/// between (may contain spaces). `None` for anything that doesn't fit this shape.
#[must_use]
pub fn parse_rule_spec(spec: &str) -> Option<(String, String, String)> {
    let mut parts = spec.trim().splitn(2, char::is_whitespace);
    let field = parts.next()?.to_lowercase();
    let rest = parts.next()?.trim();

    let match_field = match field.as_str() {
        "subject" => "subject",
        "from" | "sender" | "from_addr" => "from_addr",
        _ => return None,
    };

    let split = rest.rfind(char::is_whitespace)?;
    let (pattern, action) = (rest[..split].trim(), rest[split..].trim());
    if pattern.is_empty() {
        return None;
    }
    let action = match action.to_lowercase().as_str() {
        "star" => "star",
        "read" => "read",
        "archive" => "archive",
        "delete" => "delete",
        _ => return None,
    };

    Some((
        match_field.to_string(),
        pattern.to_string(),
        action.to_string(),
    ))
}
