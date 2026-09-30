//! macOS `LaunchAgent` that runs `triptych notify check` every minute, so deadline alerts fire with
//! the TUI closed. `install`/`uninstall` block and touch only the user's own `~/Library/LaunchAgents`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

/// launchd label, also the plist file stem.
pub const LABEL: &str = "com.triptych.notify";
const INTERVAL_SECS: u32 = 60;

/// The file behind a `sqlite:<path>` URL, made absolute against `cwd`. `None` for in-memory URLs.
#[must_use]
pub fn db_file(url: &str, cwd: &Path) -> Option<PathBuf> {
    let rest = url.strip_prefix("sqlite:")?;
    let path = rest.split('?').next()?.trim_start_matches("//");
    if path.is_empty() || path.starts_with(':') {
        return None;
    }
    Some(cwd.join(path))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The `LaunchAgent` plist. `workdir` is where `.env` is found; `db` is pinned through `DATABASE_URL`
/// because launchd's own cwd is `/`, where the default relative `todo.db` does not exist.
#[must_use]
pub fn plist(exe: &Path, workdir: &Path, db: &Path, log: &Path) -> String {
    let s = |p: &Path| xml_escape(&p.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>notify</string><string>check</string></array>
  <key>WorkingDirectory</key><string>{workdir}</string>
  <key>EnvironmentVariables</key>
  <dict><key>DATABASE_URL</key><string>sqlite:{db}</string></dict>
  <key>StartInterval</key><integer>{INTERVAL_SECS}</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        exe = s(exe),
        workdir = s(workdir),
        db = s(db),
        log = s(log),
    )
}

fn agent_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn launchctl(args: &[&str]) -> Result<Output> {
    Command::new("launchctl")
        .args(args)
        .output()
        .context("running launchctl")
}

/// launchd's per-user domain (`gui/<uid>`).
fn domain() -> Result<String> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("running `id -u`")?;
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&out.stdout).trim()
    ))
}

/// Writes the plist and loads it (replacing a loaded copy). Returns the plist path.
///
/// # Errors
///
/// Fails off macOS, when the database file does not exist (an agent started elsewhere would
/// silently create an empty one), or when writing the plist or running `launchctl` fails.
pub fn install() -> Result<PathBuf> {
    if !cfg!(target_os = "macos") {
        bail!("install is macOS only; run `triptych notify check` from cron or a systemd timer");
    }
    let cwd = std::env::current_dir()?;
    let url = crate::app::db_url();
    let db = db_file(&url, &cwd)
        .filter(|p| p.exists())
        .with_context(|| {
            format!("no database file behind {url}; run from the directory holding todo.db")
        })?;
    let path = agent_path()?;
    let log = std::env::temp_dir().join("triptych-notify.log");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, plist(&std::env::current_exe()?, &cwd, &db, &log))?;

    let target = domain()?;
    // Fails harmlessly when no copy is loaded.
    launchctl(&["bootout", &format!("{target}/{LABEL}")])?;
    let out = launchctl(&["bootstrap", &target, &path.to_string_lossy()])?;
    if !out.status.success() {
        bail!(
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(path)
}

/// Unloads the agent and deletes its plist. Returns whether a plist existed.
///
/// # Errors
///
/// Returns an error if `launchctl` cannot run or the plist cannot be removed.
pub fn uninstall() -> Result<bool> {
    if cfg!(target_os = "macos") {
        launchctl(&["bootout", &format!("{}/{LABEL}", domain()?)])?;
    }
    match std::fs::remove_file(agent_path()?) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
