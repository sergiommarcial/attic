//! `attic daemon`, `attic scan` and `attic status`.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use attic_core::events::LAST_EVENT_ID;
use attic_core::fsevents::{FsEventSource, current_event_id};
use attic_core::notify::OsascriptNotifier;
use attic_core::scan::{full_scan, now};
use attic_core::service::{Service, load_config, roots};
use attic_core::store::Store;
use attic_core::units::{age, human};
use attic_core::{AtticError, Paths, Result};

use crate::launchd::{SERVICE_LABELS, is_homebrew};
use crate::tui::ui::display_path;

/// How long FSEvents gathers events into one batch.
const LATENCY: Duration = Duration::from_secs(5);

/// The event id to resume from. A first start stores the system's current id
/// first, so a stored id always exists and the service's pending folders
/// always have a floor to replay from.
fn resume_id(store: &mut Store) -> Result<u64> {
    if let Some(id) = store.meta(LAST_EVENT_ID)?.and_then(|v| v.parse().ok()) {
        return Ok(id);
    }
    let id = current_event_id();
    store.set_meta(LAST_EVENT_ID, &id.to_string())?;
    Ok(id)
}

pub fn daemon(paths: &Paths) -> Result<()> {
    // Background work: let everything else on the machine go first.
    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
    let mut store = Store::open(&paths.db)?;
    let cfg = load_config(paths);
    let since = resume_id(&mut store)?;
    // Started before the first scan, so changes made during it are queued.
    let events = FsEventSource::start(&roots(&cfg, &paths.home), Some(since), LATENCY)?;
    let notifier = OsascriptNotifier;
    let clock = now;
    Service::new(store, paths.clone(), Box::new(events), &notifier, &clock)?.run()?;
    // run() returns when the roots changed: start over in this same process,
    // since launchd only restarts the agent after a failure.
    let exe = std::env::current_exe().map_err(|e| AtticError::io("current executable", e))?;
    let err = std::process::Command::new(&exe).arg("daemon").exec();
    Err(AtticError::io(exe, err))
}

pub fn scan(paths: &Paths) -> Result<()> {
    scan_with(paths, agent_loaded())
}

/// A foreground scan would replace rows with a snapshot minutes old while
/// the service keeps them current, so it only runs without the service.
pub fn scan_with(paths: &Paths, service_running: bool) -> Result<()> {
    if service_running {
        return Err(AtticError::Command {
            program: "scan".into(),
            message: "the background service is running and already keeps the database current; see `attic status`".into(),
        });
    }
    let mut store = Store::open(&paths.db)?;
    let cfg = load_config(paths);
    let mut stderr = std::io::stderr();
    full_scan(&mut store, &roots(&cfg, &paths.home), &cfg, &mut |p| {
        let _ = write!(
            stderr,
            "\r\x1b[2KScanning: {} folders, {}",
            p.dirs,
            human(p.bytes)
        );
    })?;
    let _ = writeln!(stderr);
    println!(
        "Scanned {} folders and files into {}.",
        store.count()?,
        paths.db.display()
    );
    Ok(())
}

pub fn status(paths: &Paths) -> Result<()> {
    println!(
        "Background service: {}",
        if agent_loaded() {
            "running"
        } else {
            "not running (run `attic install`)"
        }
    );
    if !paths.db.exists() {
        println!(
            "No scan yet. Run `attic scan`, or `attic install` to run attic in the background."
        );
        return Ok(());
    }
    let store = Store::open(&paths.db)?;
    match store
        .meta("full_scan_done_at")?
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(at) => println!("Last full scan: {}", ago(at)),
        None => println!("Last full scan: none finished yet"),
    }
    println!("Entries: {}", store.count()?);
    let unreadable = store.unreadable()?;
    if !unreadable.is_empty() {
        println!("\nCould not read {} folders:", unreadable.len());
        for line in unreadable_lines(&unreadable, &paths.home) {
            println!("{line}");
        }
        println!("\n{}", full_disk_access_hint());
    }
    Ok(())
}

/// One line per folder, with `~` and control characters made safe.
pub fn unreadable_lines(paths: &[PathBuf], home: &Path) -> Vec<String> {
    paths
        .iter()
        .map(|p| format!("  {}", display_path(p, home)))
        .collect()
}

fn ago(at: i64) -> String {
    match age(((now() - at).max(0) / 86_400) as u64).as_str() {
        "today" => "today".into(),
        other => format!("{other} ago"),
    }
}

pub fn agent_loaded() -> bool {
    let uid = unsafe { libc::getuid() };
    SERVICE_LABELS.iter().any(|label| {
        Command::new("launchctl")
            .args(["print", &format!("gui/{uid}/{label}")])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

pub fn full_disk_access_hint() -> String {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize());
    let shown = exe
        .as_ref()
        .map_or_else(|_| "attic".into(), |p| p.display().to_string());
    let again = if exe.as_deref().is_ok_and(is_homebrew) {
        " Each `brew upgrade` installs a new binary, so grant it again after upgrading."
    } else {
        ""
    };
    format!(
        "Grant Full Disk Access to {shown} in System Settings, Privacy & Security, Full Disk Access.{again}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn scan_is_refused_while_the_service_runs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(dir.path());
        let err = scan_with(&paths, true).unwrap_err();
        assert!(
            err.to_string().contains("background service is running"),
            "{err}"
        );
        assert!(!paths.db.exists(), "nothing was written");
    }

    #[test]
    fn a_first_start_stores_the_streams_current_event_id() {
        let mut store = Store::open_in_memory().unwrap();
        let id = resume_id(&mut store).unwrap();
        assert!(id > 0);
        assert_eq!(
            store.meta(LAST_EVENT_ID).unwrap().as_deref(),
            Some(id.to_string().as_str())
        );
    }

    #[test]
    fn a_stored_event_id_is_kept() {
        let mut store = Store::open_in_memory().unwrap();
        store.set_meta(LAST_EVENT_ID, "1234").unwrap();
        assert_eq!(resume_id(&mut store).unwrap(), 1234);
        assert_eq!(store.meta(LAST_EVENT_ID).unwrap().as_deref(), Some("1234"));
    }

    #[test]
    fn unreadable_folders_print_one_per_line() {
        let lines = unreadable_lines(&[PathBuf::from("/h/a\nb")], Path::new("/h"));
        assert_eq!(lines, ["  ~/a?b"]);
    }
}
