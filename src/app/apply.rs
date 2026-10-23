//! Approval-gated job applications. A sidecar (`TRIPTYCH_APPLY_CMD`) fills the form in a browser and
//! waits; nothing is submitted until the user approves in the review popup (`y`).
//!
//! Protocol, one JSON object per line. App to sidecar on stdin: `{"cmd":"fill","url":..}`,
//! `{"cmd":"recheck"}`, `{"cmd":"submit"}`, `{"cmd":"abort"}` (close the browser and exit). Sidecar to
//! app on stdout: `filled{ats,filled[],unanswered[],screenshot}`, `submitted`, `unsupported{ats}`,
//! `error{message}`. `unanswered` lists REQUIRED fields the sidecar left empty; `recheck` rescans the
//! page after the user filled them in by hand and answers with a fresh `filled`.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout, Command as Process};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::timeout;

use super::App;

/// Program (and arguments) that speaks the protocol; unset turns `A` into "open the link".
pub const CMD_ENV: &str = "TRIPTYCH_APPLY_CMD";
/// `1` lets `y` submit; anything else keeps the flow review-only.
pub const SUBMIT_ENV: &str = "TRIPTYCH_APPLY_SUBMIT";
/// Applications allowed per rolling 24 hours (default 5).
pub const MAX_PER_DAY_ENV: &str = "TRIPTYCH_APPLY_MAX_PER_DAY";

const DEFAULT_MAX_PER_DAY: i64 = 5;
const FILL_LIMIT: Duration = Duration::from_secs(120);
const SUBMIT_LIMIT: Duration = Duration::from_secs(120);
const APPROVAL_LIMIT: Duration = Duration::from_mins(15);
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// Sidecar text reaches the terminal, so each item is cut and the lists are capped.
const TEXT_MAX_CHARS: usize = 120;
const LIST_MAX: usize = 20;
const ABORT_LINE: &str = r#"{"cmd":"abort"}"#;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ApplyEvent {
    Filled {
        #[serde(default)]
        ats: String,
        #[serde(default)]
        filled: Vec<String>,
        #[serde(default)]
        unanswered: Vec<String>,
        screenshot: Option<String>,
    },
    Submitted,
    Unsupported {
        #[serde(default)]
        ats: String,
    },
    Error {
        #[serde(default)]
        message: String,
    },
}

/// One sidecar line. Its text comes from web pages, so control characters and line breaks are
/// stripped before it can reach the screen.
///
/// # Errors
/// A message when the line is not a known event.
pub fn parse_event(line: &str) -> Result<ApplyEvent, String> {
    let event: ApplyEvent =
        serde_json::from_str(line).map_err(|e| format!("bad sidecar reply: {e}"))?;
    Ok(match event {
        ApplyEvent::Filled {
            ats,
            filled,
            unanswered,
            screenshot,
        } => ApplyEvent::Filled {
            ats: clean(&ats),
            filled: clean_all(&filled),
            unanswered: clean_all(&unanswered),
            screenshot: screenshot.map(|s| clean(&s)),
        },
        ApplyEvent::Unsupported { ats } => ApplyEvent::Unsupported { ats: clean(&ats) },
        ApplyEvent::Error { message } => ApplyEvent::Error {
            message: clean(&message),
        },
        ApplyEvent::Submitted => ApplyEvent::Submitted,
    })
}

fn clean(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line
        .chars()
        .filter(|c| !c.is_control())
        .take(TEXT_MAX_CHARS)
        .collect()
}

fn clean_all(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|s| clean(s))
        .filter(|s| !s.is_empty())
        .take(LIST_MAX)
        .collect()
}

/// Whether `y` may submit (`TRIPTYCH_APPLY_SUBMIT=1`).
#[must_use]
pub fn submit_enabled() -> bool {
    std::env::var(SUBMIT_ENV).is_ok_and(|v| v.trim() == "1")
}

fn max_per_day() -> i64 {
    std::env::var(MAX_PER_DAY_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_MAX_PER_DAY)
}

#[derive(Debug)]
enum Command {
    Recheck,
    Submit,
}

#[derive(Debug)]
pub enum ApplyStage {
    /// The sidecar is filling or rechecking the page.
    Working,
    Review {
        ats: String,
        filled: Vec<String>,
        unanswered: Vec<String>,
        screenshot: Option<String>,
    },
    /// `submit` was sent and the click may already have happened, so it can no longer be cancelled.
    Submitting,
}

/// The one application in flight. Dropping it closes `cmds`, which the sidecar task treats as a cancel.
#[derive(Debug)]
pub struct ApplySession {
    posting_id: i64,
    url: String,
    pub label: String,
    pub stage: ApplyStage,
    cmds: UnboundedSender<Command>,
}

#[derive(Debug)]
pub struct ApplyResult {
    pub posting_id: i64,
    pub event: ApplyEvent,
}

impl App {
    /// `A`: starts an application for the selected `Apply:` task. Without `TRIPTYCH_APPLY_CMD`, or on a
    /// task that is not a job posting, it opens the first link like `o`.
    pub async fn start_apply(&mut self) {
        let Some(cmd) = std::env::var(CMD_ENV).ok().filter(|c| !c.trim().is_empty()) else {
            self.open_first_link();
            return;
        };
        let Some(task_id) = self.tasks.get(self.selected).map(|t| t.id) else {
            return;
        };
        let row = sqlx::query_as::<_, (i64, String, String, Option<String>, String)>(
            "SELECT id, company, role, url, status FROM job_postings WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.db_pool)
        .await;
        let (id, company, role, url, status) = match row {
            Ok(Some(row)) => row,
            Ok(None) => {
                self.open_first_link();
                return;
            }
            Err(e) => {
                self.notify(&format!("Error: {e}"));
                return;
            }
        };
        let Some(url) = url else {
            self.notify("No apply link on this posting");
            return;
        };
        if status == "applied" {
            self.notify("Already applied");
            return;
        }
        let since = crate::clock::now() - chrono::Duration::hours(24);
        let done: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job_postings WHERE applied_at > ?")
                .bind(since)
                .fetch_one(&self.db_pool)
                .await
                .unwrap_or(0);
        if done >= max_per_day() {
            self.notify(&format!(
                "Daily limit reached ({done}); raise {MAX_PER_DAY_ENV} to apply more"
            ));
            return;
        }

        let label = format!("{company} - {role}");
        tracing::info!("[Apply] posting {id} ({label}): started");
        self.set_posting_status(id, "filling").await;
        let (cmds, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(run_sidecar(cmd, url.clone(), rx, self.apply_tx.clone(), id));
        self.apply = Some(ApplySession {
            posting_id: id,
            url,
            label,
            stage: ApplyStage::Working,
            cmds,
        });
    }

    /// `y` in the review popup. The guards live here, not in the sidecar.
    pub fn submit_apply(&mut self) {
        let open = match self.apply.as_ref().map(|s| &s.stage) {
            Some(ApplyStage::Review { unanswered, .. }) => unanswered.len(),
            _ => return,
        };
        if !submit_enabled() {
            self.notify(&format!("Submit is off: set {SUBMIT_ENV}=1 to allow it"));
        } else if open > 0 {
            self.notify(&format!(
                "{open} required field(s) open: answer them in the browser, then press r"
            ));
        } else if let Some(session) = self.apply.as_mut() {
            session.stage = ApplyStage::Submitting;
            let _ = session.cmds.send(Command::Submit);
        }
    }

    /// `r` in the review popup: the sidecar rescans the page after the user's manual answers.
    pub fn recheck_apply(&mut self) {
        if let Some(session) = self.apply.as_mut()
            && matches!(session.stage, ApplyStage::Review { .. })
        {
            session.stage = ApplyStage::Working;
            let _ = session.cmds.send(Command::Recheck);
        }
    }

    /// `n`/`Esc`: closes the browser and puts the posting back to `new`.
    pub async fn cancel_apply(&mut self) {
        if self
            .apply
            .as_ref()
            .is_some_and(|s| matches!(s.stage, ApplyStage::Submitting))
        {
            self.notify("Submitting, please wait");
            return;
        }
        let Some(session) = self.apply.take() else {
            return;
        };
        tracing::info!("[Apply] posting {}: cancelled", session.posting_id);
        self.set_posting_status(session.posting_id, "new").await;
        self.notify("Application cancelled");
    }

    /// Applies one sidecar event; a result for a session the user already cancelled is dropped.
    pub async fn apply_apply_result(&mut self, done: ApplyResult) {
        let id = done.posting_id;
        let Some(mut session) = self.apply.take_if(|s| s.posting_id == id) else {
            return;
        };
        match done.event {
            ApplyEvent::Filled {
                ats,
                filled,
                unanswered,
                screenshot,
            } => {
                session.stage = ApplyStage::Review {
                    ats,
                    filled,
                    unanswered,
                    screenshot,
                };
                self.apply = Some(session);
                self.set_posting_status(id, "filled").await;
            }
            ApplyEvent::Submitted => {
                tracing::info!("[Apply] posting {id} ({}): submitted", session.label);
                let marked = sqlx::query(
                    "UPDATE job_postings SET status = 'applied', applied_at = ? WHERE id = ?",
                )
                .bind(crate::clock::now())
                .bind(id)
                .execute(&self.db_pool)
                .await;
                if let Err(e) = marked {
                    tracing::warn!("[Apply] posting {id} submitted but not recorded: {e}");
                }
                let _ = sqlx::query(
                    "UPDATE tasks SET completed = true \
                     WHERE id = (SELECT task_id FROM job_postings WHERE id = ?)",
                )
                .bind(id)
                .execute(&self.db_pool)
                .await;
                let _ = self.reload_tasks_keep_selection().await;
                self.notify(&format!("Applied: {}", session.label));
            }
            ApplyEvent::Unsupported { ats } => {
                tracing::info!("[Apply] posting {id}: unsupported site {ats}");
                self.set_posting_status(id, "new").await;
                // `open_link` writes the one status line, so the reason has to come after it.
                self.open_link(&session.url);
                self.notify("Site not supported for auto-fill, opened the link");
            }
            ApplyEvent::Error { message } => {
                tracing::warn!("[Apply] posting {id} ({}): {message}", session.label);
                self.set_posting_status(id, "failed").await;
                self.notify(&format!("Apply failed: {message}"));
            }
        }
    }

    async fn set_posting_status(&self, id: i64, status: &str) {
        let result = sqlx::query("UPDATE job_postings SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.db_pool)
            .await;
        if let Err(e) = result {
            tracing::warn!("[Apply] posting {id}: status {status} not saved: {e}");
        }
    }
}

type Replies = Lines<BufReader<ChildStdout>>;

/// Owns the sidecar process for one application and reports each event over `tx`.
async fn run_sidecar(
    cmd: String,
    url: String,
    mut cmds: UnboundedReceiver<Command>,
    tx: UnboundedSender<ApplyResult>,
    id: i64,
) {
    let emit = |event| {
        let _ = tx.send(ApplyResult {
            posting_id: id,
            event,
        });
    };
    let fail = |message: String| ApplyEvent::Error { message };
    // shortcut: split on whitespace, so the command cannot contain a path with spaces; quote-aware
    // splitting when someone needs one.
    let mut parts = cmd.split_whitespace();
    let Some(program) = parts.next() else {
        emit(fail("empty command".into()));
        return;
    };
    let spawned = Process::new(program)
        .args(parts)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            emit(fail(format!("cannot start {program}: {e}")));
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        emit(fail("sidecar has no pipes".into()));
        return;
    };
    let mut replies = BufReader::new(stdout).lines();

    converse(&mut stdin, &mut replies, &url, &mut cmds, &emit).await;

    // Ask for a clean browser shutdown; `kill_on_drop` is the backstop.
    let _ = send_line(&mut stdin, ABORT_LINE).await;
    let _ = timeout(CLOSE_GRACE, child.wait()).await;
}

/// Fill, then wait for the user's decision, rescanning on request, until a final event.
async fn converse(
    stdin: &mut ChildStdin,
    replies: &mut Replies,
    url: &str,
    cmds: &mut UnboundedReceiver<Command>,
    emit: &(impl Fn(ApplyEvent) + Sync),
) {
    let closed = || ApplyEvent::Error {
        message: "sidecar closed its input".into(),
    };
    let mut line = serde_json::json!({"cmd": "fill", "url": url}).to_string();
    loop {
        if send_line(stdin, &line).await.is_err() {
            emit(closed());
            return;
        }
        let event = next_event(replies, FILL_LIMIT).await;
        let filled = matches!(event, ApplyEvent::Filled { .. });
        emit(event);
        if !filled {
            return;
        }
        match timeout(APPROVAL_LIMIT, cmds.recv()).await {
            Ok(Some(Command::Recheck)) => line = r#"{"cmd":"recheck"}"#.into(),
            Ok(Some(Command::Submit)) => {
                if send_line(stdin, r#"{"cmd":"submit"}"#).await.is_err() {
                    emit(closed());
                } else {
                    emit(next_event(replies, SUBMIT_LIMIT).await);
                }
                return;
            }
            Ok(None) => return,
            Err(_) => {
                emit(ApplyEvent::Error {
                    message: "no decision in 15 minutes, application closed".into(),
                });
                return;
            }
        }
    }
}

async fn send_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

async fn next_event(replies: &mut Replies, limit: Duration) -> ApplyEvent {
    let error = |message: String| ApplyEvent::Error { message };
    match timeout(limit, replies.next_line()).await {
        Err(_) => error("sidecar timed out".into()),
        Ok(Ok(Some(line))) => parse_event(&line).unwrap_or_else(error),
        Ok(Ok(None)) => error("sidecar exited".into()),
        Ok(Err(e)) => error(format!("sidecar read failed: {e}")),
    }
}
