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
            .env_remove("CANVAS_ICS_URL")
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
fn repeating_task_lists_its_rule_and_done_adds_the_next_one() {
    let sb = Sandbox::new();
    let add = sb.run(&["add", "water plants every friday"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));
    let id = extract_id(&stdout(&add)).expect("task id in add output");

    let before = stdout(&sb.run(&["list"]));
    assert!(before.contains("[↻ fri]"), "rule not listed: {before}");
    assert!(
        !before.contains("every"),
        "phrase left in the title: {before}"
    );

    let done = sb.run(&["done", &id.to_string()]);
    assert!(done.status.success(), "done failed: {}", stderr(&done));

    let after = stdout(&sb.run(&["list", "--all"]));
    assert_eq!(after.matches("water plants").count(), 2, "{after}");
    assert_eq!(after.matches("[↻ fri]").count(), 2, "{after}");
}

#[test]
fn list_hides_completed_tasks_unless_all() {
    let sb = Sandbox::new();
    let add = sb.run(&["add", "Buy milk"]);
    let id = extract_id(&stdout(&add)).expect("task id in add output");
    sb.run(&["add", "Walk dog"]);
    assert!(sb.run(&["done", &id.to_string()]).status.success());

    let open = stdout(&sb.run(&["list"]));
    assert!(!open.contains("Buy milk"), "completed task listed: {open}");
    assert!(open.contains("Walk dog"), "open task missing: {open}");

    let all = stdout(&sb.run(&["list", "-a"]));
    assert!(all.contains("Buy milk"), "--all missing completed: {all}");
    assert!(all.contains("Walk dog"), "--all missing open: {all}");
}

#[test]
fn list_with_only_completed_tasks_points_at_all() {
    let sb = Sandbox::new();
    let add = sb.run(&["add", "Buy milk"]);
    let id = extract_id(&stdout(&add)).expect("task id in add output");
    assert!(sb.run(&["done", &id.to_string()]).status.success());

    let out = stdout(&sb.run(&["list"]));
    assert!(out.contains("No open tasks"), "{out}");
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
fn a_pinned_clock_decides_what_tomorrow_is() {
    let sb = Sandbox::new();
    let run = |args: &[&str]| {
        sb.cmd(args)
            .env("TRIPTYCH_NOW", "2030-03-05T12:00:00Z")
            .output()
            .expect("spawn triptych")
    };
    let add = run(&["add", "Submit report on 03/06"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));

    let list_out = stdout(&run(&["list"]));
    assert!(
        list_out.contains("[TOMORROW]"),
        "TRIPTYCH_NOW was ignored: {list_out}"
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
fn a_new_default_database_says_where_it_was_created() {
    let sb = Sandbox::new();
    let out = sb
        .cmd(&["list"])
        .env_remove("DATABASE_URL")
        .output()
        .expect("spawn triptych");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Creating a new database at") && stderr(&out).contains("todo.db"),
        "{}",
        stderr(&out)
    );
    assert!(sb.path("todo.db").exists(), "default db not in the cwd");

    let again = sb
        .cmd(&["list"])
        .env_remove("DATABASE_URL")
        .output()
        .expect("spawn triptych");
    assert!(!stderr(&again).contains("Creating a new database"));
}

#[test]
fn the_daemon_takes_a_request_larger_than_one_read() {
    let sb = Sandbox::new();
    let mut child = sb.cmd(&["daemon"]).spawn().expect("spawn daemon process");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !sb.socket_path().exists() {
        assert!(Instant::now() < deadline, "daemon never created its socket");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Past the old single 8 KB read; the daemon stores it as one task.
    let long = format!("write notes {}", "word ".repeat(2000));
    let add = sb.run(&["add", &long]);
    let _ = sb.run(&["stop"]);
    let _ = child.wait();

    assert!(add.status.success(), "{}", stderr(&add));
    // A failed daemon call falls back to direct mode, so success alone proves nothing.
    assert!(stdout(&add).contains("via daemon"), "{}", stdout(&add));
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

/// Starts `triptych daemon` with stderr captured, then waits until it accepts connections.
/// The socket exists once bound, before the accept loop runs, so a `status` round trip is the
/// readiness signal.
fn spawn_daemon(sb: &Sandbox) -> std::process::Child {
    let child = sb
        .cmd(&["daemon"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn daemon process");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !stdout(&sb.run(&["status"])).contains("Daemon is running") {
        assert!(Instant::now() < deadline, "daemon never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    child
}

#[test]
fn daemon_adds_hand_back_the_real_row_ids() {
    let sb = Sandbox::new();
    let child = spawn_daemon(&sb);

    let first = stdout(&sb.run(&["add", "first thing"]));
    let second = stdout(&sb.run(&["add", "second thing"]));
    let _ = sb.run(&["stop"]);
    let _ = child.wait_with_output();

    assert!(first.contains("via daemon"), "{first}");
    assert_eq!(extract_id(&first), Some(1), "{first}");
    assert_eq!(extract_id(&second), Some(2), "{second}");
}

#[test]
fn daemon_drops_a_client_that_connects_and_sends_nothing() {
    let sb = Sandbox::new();
    let child = spawn_daemon(&sb);

    let mut idle = std::os::unix::net::UnixStream::connect(sb.socket_path()).expect("connect");
    idle.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut reply = Vec::new();
    // EOF (Ok) means the daemon hung up; a timeout here means it kept the connection open.
    let read = std::io::Read::read_to_end(&mut idle, &mut reply);
    // Still serving everyone else.
    let status = stdout(&sb.run(&["status"]));
    let _ = sb.run(&["stop"]);
    let _ = child.wait_with_output();

    assert!(
        read.is_ok(),
        "daemon kept an idle connection open: {read:?}"
    );
    assert!(status.contains("Daemon is running"), "{status}");
}

#[test]
fn daemon_reports_how_many_cached_inputs_it_preloaded() {
    let sb = Sandbox::new();
    for input in ["alpha errand", "beta errand"] {
        let add = sb.run(&["add", input]);
        assert!(add.status.success(), "{}", stderr(&add));
    }
    let mut child = spawn_daemon(&sb);

    // The preload runs behind the accept loop, so wait for its line instead of assuming it is out.
    let pipe = child.stderr.take().expect("piped stderr");
    let (lines_tx, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufRead::lines(std::io::BufReader::new(pipe)).map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut log = String::new();
    while !log.contains("cache entries") {
        let line = lines
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("no preload line from the daemon: {log}"));
        log.push_str(&line);
        log.push('\n');
    }
    let _ = sb.run(&["stop"]);
    let _ = child.wait();

    assert!(log.contains("Loaded 2 cache entries"), "{log}");
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

/// The first day listed gets its heading, and "Allocated tasks:" appears only when there are some.
#[test]
fn schedule_show_heads_every_day_and_lists_allocations_only_when_present() {
    let sb = Sandbox::new();
    let tomorrow = weekday_name(
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
day = "tuesday"
type = "admin"
start = "12:00"
end = "13:00"
title = "Errands"

[[blocks]]
day = "{tomorrow}"
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

    let before = stdout(&sb.run(&["schedule", "show"]));
    assert!(before.contains("Tuesday:"), "{before}");
    assert!(!before.contains("Allocated tasks:"), "{before}");

    let add = sb.run(&["add", "Finish report by tomorrow 2h"]);
    assert!(add.status.success(), "add failed: {}", stderr(&add));
    let reallocate = sb.run(&["schedule", "reallocate"]);
    assert!(
        reallocate.status.success(),
        "reallocate failed: {}",
        stderr(&reallocate)
    );

    let after = stdout(&sb.run(&["schedule", "show"]));
    assert!(after.contains("Allocated tasks:"), "{after}");
    assert!(after.contains("Finish report"), "{after}");
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

#[test]
fn canvas_sync_requires_a_feed_url() {
    let sb = Sandbox::new();
    let out = sb.run(&["canvas", "sync"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Canvas not configured"),
        "unexpected error: {}",
        stderr(&out)
    );
}

#[test]
fn canvas_sync_imports_assignments_once() {
    use std::io::{Read, Write};
    let feed = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a1\r\nSUMMARY:Essay 2\r\n\
DTSTART:20991015T235900Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/feed.ics", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = conn.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{feed}",
                feed.len()
            );
            conn.write_all(resp.as_bytes()).unwrap();
        }
    });

    let sb = Sandbox::new();
    let run = |sb: &Sandbox| {
        sb.cmd(&["canvas", "sync"])
            .env("CANVAS_ICS_URL", &url)
            .output()
            .expect("spawn triptych")
    };
    let first = run(&sb);
    assert!(first.status.success(), "sync failed: {}", stderr(&first));
    assert!(
        stdout(&first).contains("1 added, 0 updated"),
        "{}",
        stdout(&first)
    );
    let second = run(&sb);
    assert!(
        stdout(&second).contains("0 added, 0 updated"),
        "{}",
        stdout(&second)
    );
    server.join().unwrap();

    let list = sb.run(&["list"]);
    assert!(stdout(&list).contains("Essay 2"), "{}", stdout(&list));
    assert_eq!(stdout(&list).matches("Essay 2").count(), 1);
}

/// Inserts an open task due `minutes` from now, bypassing the NLP parser.
fn seed_task_due_in(db_path: &Path, description: &str, minutes: i64) {
    let rt = tokio::runtime::Runtime::new().expect("build tokio runtime");
    rt.block_on(async {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
            .await
            .expect("connect to sandbox db");
        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, deadline) VALUES (?, 0, 0, 1, ?)",
        )
        .bind(description)
        .bind(Utc::now() + chrono::Duration::minutes(minutes))
        .execute(&pool)
        .await
        .expect("seed task");
    });
}

/// A `TRIPTYCH_NOTIFY_CMD` script that appends `title|body` to the returned log path.
fn alert_logger(sb: &Sandbox) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (script, log) = (sb.path("alert.sh"), sb.path("alerts.log"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> '{}'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

#[test]
fn notify_check_alerts_once_and_needs_no_tui() {
    let sb = Sandbox::new();
    let (script, log) = alert_logger(&sb);
    let check = || {
        sb.cmd(&["notify", "check"])
            .env("TRIPTYCH_NOTIFY_CMD", &script)
            .output()
            .expect("spawn triptych")
    };

    let fresh = check();
    assert!(fresh.status.success(), "{}", stderr(&fresh));
    assert!(
        stdout(&fresh).contains("No alerts due"),
        "{}",
        stdout(&fresh)
    );

    seed_task_due_in(&sb.db_path(), "Essay due soon", 30);
    seed_task_due_in(&sb.db_path(), "Far away", 5 * 24 * 60);
    let first = check();
    assert!(
        stdout(&first).contains("Sent 1 alert"),
        "{}",
        stdout(&first)
    );
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("Due within 1 hour|Essay due soon"),
        "{logged}"
    );
    assert!(!logged.contains("Far away"), "{logged}");

    let second = check();
    assert!(
        stdout(&second).contains("No alerts due"),
        "{}",
        stdout(&second)
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[test]
fn notify_check_honors_the_off_switch() {
    let sb = Sandbox::new();
    let (script, log) = alert_logger(&sb);
    assert!(sb.run(&["list"]).status.success());
    seed_task_due_in(&sb.db_path(), "Essay due soon", 30);
    let out = sb
        .cmd(&["notify", "check"])
        .env("TRIPTYCH_NOTIFY_CMD", &script)
        .env("TRIPTYCH_NOTIFY", "off")
        .output()
        .expect("spawn triptych");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("Alerts are off"), "{}", stdout(&out));
    assert!(!log.exists(), "alerted while off");
}

#[test]
fn notify_check_retries_an_alert_whose_notifier_failed() {
    let sb = Sandbox::new();
    let (script, log) = alert_logger(&sb);
    assert!(sb.run(&["list"]).status.success());
    seed_task_due_in(&sb.db_path(), "Essay due soon", 30);
    let check = |program: &Path| {
        sb.cmd(&["notify", "check"])
            .env("TRIPTYCH_NOTIFY_CMD", program)
            .output()
            .expect("spawn triptych")
    };

    let failed = check(Path::new("false"));
    assert!(
        stderr(&failed).contains("1 alert(s) could not be shown"),
        "{}",
        stderr(&failed)
    );
    assert!(!stdout(&failed).contains("Sent"), "{}", stdout(&failed));

    let retried = check(&script);
    assert!(
        stdout(&retried).contains("Sent 1 alert"),
        "{}",
        stdout(&retried)
    );
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("Essay due soon")
    );
}

#[test]
fn notify_install_refuses_without_a_database() {
    let sb = Sandbox::new();
    // HOME points into the sandbox so nothing can reach the real ~/Library/LaunchAgents.
    let out = sb
        .cmd(&["notify", "install"])
        .env("HOME", &sb.dir)
        .output()
        .expect("spawn triptych");
    assert!(!out.status.success());
    assert!(stderr(&out).contains('✗'), "{}", stderr(&out));
    assert!(!sb.path("Library").exists(), "wrote a plist anyway");
}
