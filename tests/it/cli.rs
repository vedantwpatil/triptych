//! Integration tests that drive the compiled `triptych` binary as a real user would -
//! `Command::new(env!("CARGO_BIN_EXE_triptych"))`, not an in-process call into `App`.
//!
//! Each test gets its own throwaway `Sandbox`: a unique temp directory holding an
//! isolated `DATABASE_URL` sqlite file and `TRIPTYCH_SOCKET_PATH` daemon socket, passed
//! to the child process via `Command::env` (not `std::env::set_var`, so this stays safe
//! under `cargo test`'s default parallel test execution). This never touches the real
//! `todo.db` or a real running daemon - see docs/DEVELOPMENT.md's "Integration tests"
//! section for why that isolation matters here specifically.
//!
//! Email/IMAP env vars are explicitly stripped from every child process so these tests
//! are deterministic regardless of what's `source`d in the shell that runs `cargo test`.

use chrono::{Datelike, TimeZone, Utc, Weekday};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Sandbox {
    dir: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "triptych_test_{}_{}_{}",
            std::process::id(),
            nanos,
            n
        ));
        std::fs::create_dir_all(&dir).expect("create sandbox dir");
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn socket_path(&self) -> PathBuf {
        self.path("test.sock")
    }

    fn db_path(&self) -> PathBuf {
        self.path("test.db")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_triptych"));
        cmd.args(args)
            .current_dir(&self.dir)
            .env(
                "DATABASE_URL",
                format!("sqlite:{}", self.db_path().display()),
            )
            .env("TRIPTYCH_SOCKET_PATH", self.socket_path())
            .env_remove("TRIPTYCH_EMAIL_ENABLED")
            .env_remove("IMAP_ACCOUNTS")
            .env_remove("IMAP_SERVER")
            .env_remove("IMAP_USERNAME")
            .env_remove("IMAP_PASSWORD")
            .env_remove("IMAP_PORT")
            .env_remove("IMAP_FOLDER");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("spawn triptych")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Pulls the numeric id out of `"... (ID: 42)..."`, as printed by `add`/`list`.
fn extract_id(text: &str) -> Option<i64> {
    let idx = text.find("(ID: ")?;
    let rest = &text[idx + "(ID: ".len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

/// Lowercase full-name form `App::parse_days` accepts for a single weekday.
const fn weekday_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

struct SeedEmail<'a> {
    uid: i64,
    message_id: &'a str,
    account: &'a str,
    folder: &'a str,
    from_addr: &'a str,
    from_name: Option<&'a str>,
    subject: &'a str,
    date_utc: chrono::DateTime<Utc>,
    is_read: bool,
}

/// Inserts rows directly into `email_messages`, bypassing IMAP entirely, so email-list
/// formatting can be tested without a real mailbox. Runs its own Tokio runtime since this
/// file's tests are otherwise plain sync `#[test]`s driving a subprocess.
fn seed_emails(db_path: &Path, rows: &[SeedEmail]) {
    let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
    rt.block_on(async {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
            .await
            .expect("connect to sandbox db");
        for row in rows {
            sqlx::query(
                "INSERT INTO email_messages (uid, message_id, account, folder, from_addr, from_name, subject, date_utc, is_read) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(row.uid)
            .bind(row.message_id)
            .bind(row.account)
            .bind(row.folder)
            .bind(row.from_addr)
            .bind(row.from_name)
            .bind(row.subject)
            .bind(row.date_utc)
            .bind(row.is_read)
            .execute(&pool)
            .await
            .expect("insert seed email");
        }
        pool.close().await;
    });
}

#[test]
fn add_list_done_rm_clear_roundtrip() {
    let sb = Sandbox::new();

    let add1 = sb.run(&["add", "Buy milk"]);
    assert!(add1.status.success(), "add failed: {}", stderr(&add1));
    let id1 = extract_id(&stdout(&add1)).expect("task id in add output");

    let add2 = sb.run(&["add", "Walk dog"]);
    assert!(add2.status.success(), "add failed: {}", stderr(&add2));
    let id2 = extract_id(&stdout(&add2)).expect("task id in add output");

    let list1 = stdout(&sb.run(&["list"]));
    assert!(list1.contains("Buy milk"), "list missing task: {list1}");
    assert!(list1.contains("Walk dog"), "list missing task: {list1}");

    let done = sb.run(&["done", &id1.to_string()]);
    assert!(done.status.success(), "done failed: {}", stderr(&done));

    let rm = sb.run(&["rm", &id2.to_string()]);
    assert!(rm.status.success(), "rm failed: {}", stderr(&rm));

    let list2 = stdout(&sb.run(&["list"]));
    assert!(
        !list2.contains("Walk dog"),
        "removed task still listed: {list2}"
    );

    let clear = sb.run(&["clear"]);
    assert!(clear.status.success(), "clear failed: {}", stderr(&clear));
    assert!(stdout(&clear).contains("Cleared 1 completed task"));

    let list3 = stdout(&sb.run(&["list"]));
    assert!(
        !list3.contains("Buy milk"),
        "cleared task still listed: {list3}"
    );
}

#[test]
fn done_and_rm_report_missing_ids() {
    let sb = Sandbox::new();

    let done = sb.run(&["done", "999"]);
    assert!(!done.status.success(), "done on missing id should fail");
    assert!(stderr(&done).contains("not found"));

    let rm = sb.run(&["rm", "999"]);
    assert!(!rm.status.success(), "rm on missing id should fail");
    assert!(stderr(&rm).contains("not found"));
}

#[test]
fn nlp_parses_tags_priority_and_relative_date() {
    let sb = Sandbox::new();
    let add = sb.run(&["add", "Submit report on 12/25/2099 #work !!"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let list_out = stdout(&sb.run(&["list"]));
    assert!(list_out.contains("#work"), "tag not parsed: {list_out}");
    assert!(
        list_out.contains("[HIGH]"),
        "priority not parsed: {list_out}"
    );
    assert!(list_out.contains("[12/25]"), "date not parsed: {list_out}");
}

#[test]
fn priority_rises_when_the_date_is_near() {
    let sb = Sandbox::new();
    let add = sb.run(&["add", "Submit report tomorrow #work"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let list_out = stdout(&sb.run(&["list"]));
    assert!(
        list_out.contains("[TOMORROW]"),
        "date not parsed: {list_out}"
    );
    assert!(
        list_out.contains("[URGENT↑]"),
        "priority not raised: {list_out}"
    );
}

#[test]
fn schedule_import_show_clear_roundtrip() {
    let sb = Sandbox::new();
    // Field names must match `BlockDefinition` in src/app.rs (day/type/start/end/title) -
    // NOT the day_of_week/block_type/start_time/end_time names in README.md's example,
    // which don't match the deserializer and fail to import (see docs/DEVELOPMENT.md).
    let toml_path = sb.path("schedule.toml");
    std::fs::write(
        &toml_path,
        r#"
[[blocks]]
day = "monday"
type = "deepwork"
start = "09:00"
end = "10:30"
title = "Focus Time"
"#,
    )
    .unwrap();

    let import = sb.run(&["schedule", "import", toml_path.to_str().unwrap()]);
    assert!(
        import.status.success(),
        "import failed: {}",
        stderr(&import)
    );
    assert!(stdout(&import).contains("Imported 1 schedule block"));

    let show = stdout(&sb.run(&["schedule", "show"]));
    assert!(
        show.contains("Focus Time"),
        "block missing from show: {show}"
    );
    assert!(show.contains("Monday"));

    let export_path = sb.path("out.toml");
    let export = sb.run(&["schedule", "export", export_path.to_str().unwrap()]);
    assert!(
        export.status.success(),
        "export failed: {}",
        stderr(&export)
    );
    let exported = std::fs::read_to_string(&export_path).expect("read exported toml");
    assert!(exported.contains("Focus Time"));

    let clear = sb.run(&["schedule", "clear"]);
    assert!(clear.status.success());
    assert!(stdout(&clear).contains("Cleared 1 schedule block"));

    let show_after = stdout(&sb.run(&["schedule", "show"]));
    assert!(show_after.contains("No schedule blocks defined"));
}

#[test]
fn daemon_start_status_stop_cleans_up_socket() {
    let sb = Sandbox::new();

    let mut child = sb.cmd(&["daemon"]).spawn().expect("spawn daemon process");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !sb.socket_path().exists() {
        assert!(
            Instant::now() < deadline,
            "daemon never created its socket at {:?}",
            sb.socket_path()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let status = stdout(&sb.run(&["status"]));
    assert!(
        status.contains("Daemon is running"),
        "unexpected status: {status}"
    );

    let stop = sb.run(&["stop"]);
    assert!(stop.status.success(), "stop failed: {}", stderr(&stop));

    let exit = child.wait().expect("wait on daemon process");
    assert!(exit.success(), "daemon exited non-zero: {exit:?}");
    assert!(
        !sb.socket_path().exists(),
        "socket left behind after stop (regression: see src/cli/daemon.rs Shutdown handling)"
    );

    let status_after = stdout(&sb.run(&["status"]));
    assert!(
        status_after.contains("not running"),
        "unexpected status: {status_after}"
    );
}

#[test]
fn email_commands_fail_gracefully_without_config() {
    let sb = Sandbox::new();

    let sync = sb.run(&["email", "sync"]);
    assert!(
        !sync.status.success(),
        "sync should fail without IMAP config"
    );
    assert!(
        stderr(&sync).contains("Email not configured"),
        "unexpected error: {}",
        stderr(&sync)
    );

    let list = sb.run(&["email", "list"]);
    assert!(
        list.status.success(),
        "email list failed: {}",
        stderr(&list)
    );
    assert!(stdout(&list).contains("No emails yet"));
}

#[test]
fn schedule_reallocate_fits_deadline_task_into_available_block() {
    let sb = Sandbox::new();
    let tomorrow_day = weekday_name(
        chrono::Local::now()
            .date_naive()
            .succ_opt()
            .unwrap()
            .weekday(),
    );

    let toml_path = sb.path("schedule.toml");
    std::fs::write(
        &toml_path,
        format!(
            r#"
[[blocks]]
day = "{tomorrow_day}"
type = "deepwork"
start = "06:00"
end = "10:00"
title = "Focus Time"
"#
        ),
    )
    .unwrap();

    let import = sb.run(&["schedule", "import", toml_path.to_str().unwrap()]);
    assert!(
        import.status.success(),
        "import failed: {}",
        stderr(&import)
    );

    // "by tomorrow 2h" resolves fully via the regex fast path (deadline + explicit
    // duration, no start time) - see src/nlp/rules.rs - so this never touches Ollama.
    let add = sb.run(&["add", "Finish report by tomorrow 2h"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let reallocate = sb.run(&["schedule", "reallocate"]);
    assert!(
        reallocate.status.success(),
        "reallocate failed: {}",
        stderr(&reallocate)
    );
    assert!(
        stdout(&reallocate).contains("All deadline tasks fit within available blocks"),
        "unexpected reallocate output: {}",
        stdout(&reallocate)
    );
}

#[test]
fn schedule_reallocate_reports_conflict_with_reason() {
    let sb = Sandbox::new();

    // No schedule blocks imported - zero capacity, so this deadline task can't fit.
    let add = sb.run(&["add", "Finish report by today 2h"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let reallocate = sb.run(&["schedule", "reallocate"]);
    assert!(
        reallocate.status.success(),
        "reallocate failed: {}",
        stderr(&reallocate)
    );
    let out = stdout(&reallocate);
    assert!(
        out.contains("task(s) not scheduled: 1 out of block capacity"),
        "unexpected output: {out}"
    );
    assert!(
        out.contains("needs 120m, got 0m"),
        "unexpected output: {out}"
    );
    assert!(
        out.contains("no free deepwork/admin time before the deadline"),
        "unexpected output: {out}"
    );
}

#[test]
fn schedule_import_supports_compound_and_group_day_names() {
    let sb = Sandbox::new();
    let toml_path = sb.path("schedule.toml");
    std::fs::write(
        &toml_path,
        r#"
[[blocks]]
day = "weekdays"
type = "deepwork"
start = "09:00"
end = "10:00"
title = "Weekday Focus"

[[blocks]]
day = "monday_wednesday_friday"
type = "admin"
start = "14:00"
end = "14:30"
title = "MWF Admin"
"#,
    )
    .unwrap();

    let import = sb.run(&["schedule", "import", toml_path.to_str().unwrap()]);
    assert!(
        import.status.success(),
        "import failed: {}",
        stderr(&import)
    );
    // 5 weekday instances of the first block + 3 (Mon/Wed/Fri) of the second.
    assert!(
        stdout(&import).contains("Imported 8 schedule blocks"),
        "unexpected output: {}",
        stdout(&import)
    );

    let show = stdout(&sb.run(&["schedule", "show"]));
    assert_eq!(
        show.matches("Weekday Focus").count(),
        5,
        "expected 5 weekday instances: {show}"
    );
    assert_eq!(
        show.matches("MWF Admin").count(),
        3,
        "expected 3 Mon/Wed/Fri instances: {show}"
    );
}

#[test]
fn schedule_import_skips_overlapping_block_with_warning() {
    let sb = Sandbox::new();
    let toml_path = sb.path("schedule.toml");
    std::fs::write(
        &toml_path,
        r#"
[[blocks]]
day = "monday"
type = "deepwork"
start = "09:00"
end = "10:00"
title = "Block A"

[[blocks]]
day = "monday"
type = "deepwork"
start = "09:30"
end = "10:30"
title = "Block B"
"#,
    )
    .unwrap();

    let import = sb.run(&["schedule", "import", toml_path.to_str().unwrap()]);
    assert!(
        import.status.success(),
        "import failed: {}",
        stderr(&import)
    );
    assert!(
        stdout(&import).contains("Imported 1 schedule blocks"),
        "unexpected output: {}",
        stdout(&import)
    );
    assert!(
        stderr(&import).contains("Warning: Skipping overlapping block 'Block B' on monday"),
        "unexpected stderr: {}",
        stderr(&import)
    );

    let show = stdout(&sb.run(&["schedule", "show"]));
    assert!(show.contains("Block A"), "missing surviving block: {show}");
    assert!(
        !show.contains("Block B"),
        "overlapping block should have been skipped: {show}"
    );
}

#[test]
fn schedule_import_clear_flag_replaces_existing_blocks() {
    let sb = Sandbox::new();

    let first_toml = sb.path("first.toml");
    std::fs::write(
        &first_toml,
        r#"
[[blocks]]
day = "monday"
type = "deepwork"
start = "09:00"
end = "10:00"
title = "Old Block"
"#,
    )
    .unwrap();
    let import1 = sb.run(&["schedule", "import", first_toml.to_str().unwrap()]);
    assert!(
        import1.status.success(),
        "first import failed: {}",
        stderr(&import1)
    );

    let second_toml = sb.path("second.toml");
    std::fs::write(
        &second_toml,
        r#"
[[blocks]]
day = "tuesday"
type = "admin"
start = "14:00"
end = "15:00"
title = "New Block"
"#,
    )
    .unwrap();
    let import2 = sb.run(&[
        "schedule",
        "import",
        "--clear",
        second_toml.to_str().unwrap(),
    ]);
    assert!(
        import2.status.success(),
        "second import failed: {}",
        stderr(&import2)
    );
    assert!(
        stdout(&import2).contains("Cleared existing blocks"),
        "unexpected output: {}",
        stdout(&import2)
    );
    assert!(
        stdout(&import2).contains("Imported 1 schedule blocks"),
        "unexpected output: {}",
        stdout(&import2)
    );

    let show = stdout(&sb.run(&["schedule", "show"]));
    assert!(show.contains("New Block"), "missing new block: {show}");
    assert!(
        !show.contains("Old Block"),
        "--clear should have removed the old block: {show}"
    );
}

#[test]
fn email_list_formats_seeded_messages_with_read_marker_and_account_tag() {
    let sb = Sandbox::new();

    // Any command creates the sandbox db and runs migrations, so email_messages exists
    // before we insert into it directly below.
    let list0 = sb.run(&["list"]);
    assert!(list0.status.success(), "list failed: {}", stderr(&list0));

    let older = Utc.with_ymd_and_hms(2026, 9, 10, 9, 0, 0).unwrap();
    let newer = Utc.with_ymd_and_hms(2026, 9, 15, 12, 30, 0).unwrap();
    seed_emails(
        &sb.db_path(),
        &[
            SeedEmail {
                uid: 1,
                message_id: "msg-work-1",
                account: "work",
                folder: "INBOX",
                from_addr: "alice@example.com",
                from_name: Some("Alice"),
                subject: "Q3 Planning",
                date_utc: older,
                is_read: false,
            },
            SeedEmail {
                uid: 2,
                message_id: "msg-personal-1",
                account: "personal",
                folder: "INBOX",
                from_addr: "bob@example.com",
                from_name: None,
                subject: "Weekend Trip",
                date_utc: newer,
                is_read: true,
            },
        ],
    );

    let list = sb.run(&["email", "list"]);
    assert!(
        list.status.success(),
        "email list failed: {}",
        stderr(&list)
    );
    let out = stdout(&list);

    assert!(out.contains("(work)"), "missing account tag: {out}");
    assert!(out.contains("(personal)"), "missing account tag: {out}");
    assert!(out.contains("Alice"), "missing from_name: {out}");
    assert!(
        out.contains("bob@example.com"),
        "missing from_addr fallback: {out}"
    );
    assert!(out.contains("Q3 Planning"), "missing subject: {out}");
    assert!(out.contains("Weekend Trip"), "missing subject: {out}");

    let unread_line = out
        .lines()
        .find(|l| l.contains("Q3 Planning"))
        .expect("unread line present");
    assert!(
        unread_line.contains('*'),
        "unread message missing '*' marker: {unread_line}"
    );
    let read_line = out
        .lines()
        .find(|l| l.contains("Weekend Trip"))
        .expect("read line present");
    assert!(
        !read_line.contains('*'),
        "read message should not have '*' marker: {read_line}"
    );

    // Merged inbox orders most-recent-first regardless of account.
    let newer_pos = out.find("Weekend Trip").unwrap();
    let older_pos = out.find("Q3 Planning").unwrap();
    assert!(newer_pos < older_pos, "expected newer message first: {out}");
}

#[test]
fn email_sync_fails_gracefully_against_unreachable_server() {
    let sb = Sandbox::new();

    let mut cmd = sb.cmd(&["email", "sync"]);
    cmd.env("TRIPTYCH_EMAIL_ENABLED", "true")
        .env("IMAP_SERVER", "127.0.0.1")
        .env("IMAP_PORT", "18889")
        .env("IMAP_USERNAME", "testuser")
        .env("IMAP_PASSWORD", "testpass");
    let sync = cmd.output().expect("spawn triptych");

    assert!(
        !sync.status.success(),
        "sync against a closed port should fail"
    );
    assert!(
        stderr(&sync).contains("[default] Sync failed"),
        "unexpected error: {}",
        stderr(&sync)
    );
}
