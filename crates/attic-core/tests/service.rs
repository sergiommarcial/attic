use std::cell::Cell;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use attic_core::events::{Change, EventBatch, ScriptedEvents};
use attic_core::notify::RecordingNotifier;
use attic_core::service::{Flow, Service, roots};
use attic_core::store::Store;
use attic_core::{Config, Paths};

const T0: i64 = 2_000_000_000;

fn write_old(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = File::create(path).unwrap();
    f.write_all(&vec![3u8; bytes]).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
}

fn home() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    fs::create_dir_all(&home).unwrap();
    (dir, Paths::from_home(&home))
}

fn config(paths: &Paths, text: &str) {
    fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
    fs::write(&paths.config, text).unwrap();
}

fn change(dir: &Path, id: u64) -> EventBatch {
    EventBatch {
        changes: vec![Change {
            dir: dir.to_path_buf(),
            subtree: false,
        }],
        last_id: id,
        history_lost: false,
    }
}

#[test]
fn first_run_scans_then_applies_events() {
    let (_d, paths) = home();
    write_old(&paths.home.join("docs/a.bin"), 600_000);
    config(&paths, r#"min_file_size = "100kB""#);
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    let events = ScriptedEvents::new(vec![change(&paths.home.join("docs"), 9)]);
    let mut service = Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(events),
        &notifier,
        &clock,
    )
    .unwrap();
    service.ensure_scanned().unwrap();
    write_old(&paths.home.join("docs/b.bin"), 300_000);
    assert_eq!(service.step(Duration::ZERO).unwrap(), Flow::Continue);

    let store = Store::open(&paths.db).unwrap();
    assert!(store.meta("full_scan_done_at").unwrap().is_some());
    assert!(store.get(&paths.home.join("docs/b.bin")).unwrap().is_some());
    assert_eq!(
        store.meta("fsevents_last_id").unwrap().as_deref(),
        Some("9")
    );
}

#[test]
fn notifies_once_when_threshold_crossed() {
    let (_d, paths) = home();
    write_old(&paths.home.join("old/a.bin"), 600_000);
    config(
        &paths,
        "min_file_size = \"100kB\"\nnotify_threshold = \"200kB\"",
    );
    let notifier = RecordingNotifier::default();
    let now = Cell::new(T0);
    let clock = || now.get();
    let mut service = Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(ScriptedEvents::default()),
        &notifier,
        &clock,
    )
    .unwrap();
    service.ensure_scanned().unwrap();
    service.step(Duration::ZERO).unwrap();
    now.set(T0 + 60);
    service.step(Duration::ZERO).unwrap();

    let sent = notifier.sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "attic");
    assert!(sent[0].1.starts_with("About "), "{}", sent[0].1);
    let store = Store::open(&paths.db).unwrap();
    assert_eq!(
        store.meta("last_notified_at").unwrap().as_deref(),
        Some(T0.to_string().as_str())
    );
}

#[test]
fn config_reload_keeps_last_good_on_error() {
    let (_d, paths) = home();
    config(&paths, r#"min_file_size = "1GB""#);
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    let config_dir = paths.config.parent().unwrap().to_path_buf();
    let events = ScriptedEvents::new(vec![change(&config_dir, 1), change(&config_dir, 2)]);
    let mut service = Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(events),
        &notifier,
        &clock,
    )
    .unwrap();
    assert_eq!(service.config().min_file_size, 1_000_000_000);

    config(&paths, r#"min_file_size = "2GB""#);
    service.step(Duration::ZERO).unwrap();
    assert_eq!(service.config().min_file_size, 2_000_000_000);

    config(&paths, "min_file_size = \"lots\"");
    service.step(Duration::ZERO).unwrap();
    assert_eq!(service.config().min_file_size, 2_000_000_000);
}

#[test]
fn changed_roots_ask_for_a_restart() {
    let (d, paths) = home();
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    let config_dir = paths.config.parent().unwrap().to_path_buf();
    fs::create_dir_all(&config_dir).unwrap();
    let events = ScriptedEvents::new(vec![change(&config_dir, 1)]);
    let mut service = Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(events),
        &notifier,
        &clock,
    )
    .unwrap();
    config(
        &paths,
        &format!(
            "roots = [{:?}]",
            d.path().join("other").display().to_string()
        ),
    );
    assert_eq!(service.step(Duration::ZERO).unwrap(), Flow::Restart);
}

#[test]
fn roots_drop_nested() {
    let cfg = Config {
        roots: vec![
            PathBuf::from("/h/inside"),
            PathBuf::from("/data"),
            PathBuf::from("/data"),
        ],
        ..Config::default()
    };
    assert_eq!(
        roots(&cfg, Path::new("/h")),
        [PathBuf::from("/h"), PathBuf::from("/data")]
    );
}

#[test]
fn own_database_folder_is_excluded() {
    let (_d, paths) = home();
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    let service = Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(ScriptedEvents::default()),
        &notifier,
        &clock,
    )
    .unwrap();
    assert!(
        service
            .config()
            .excludes
            .contains(&paths.db.parent().unwrap().to_path_buf())
    );
    let loaded = attic_core::service::load_config(&paths);
    assert!(
        loaded
            .excludes
            .contains(&paths.db.parent().unwrap().to_path_buf())
    );
}

#[test]
fn the_trash_is_never_scanned() {
    let (_d, paths) = home();
    let cfg = attic_core::service::load_config(&paths);
    assert!(cfg.excludes.contains(&paths.home.join(".Trash")));
}

fn service<'a>(
    paths: &Paths,
    notifier: &'a RecordingNotifier,
    clock: &'a dyn Fn() -> i64,
    events: ScriptedEvents,
) -> Service<'a> {
    Service::new(
        Store::open(&paths.db).unwrap(),
        paths.clone(),
        Box::new(events),
        notifier,
        clock,
    )
    .unwrap()
}

#[test]
fn reconcile_scans_added_roots_and_drops_removed_ones() {
    let (d, paths) = home();
    let other = d.path().canonicalize().unwrap().join("other");
    let gone = d.path().canonicalize().unwrap().join("gone");
    write_old(&other.join("x.bin"), 300_000);
    write_old(&gone.join("y.bin"), 300_000);
    config(
        &paths,
        &format!("roots = [{:?}]", gone.display().to_string()),
    );
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    service(&paths, &notifier, &clock, ScriptedEvents::default())
        .ensure_scanned()
        .unwrap();
    assert!(
        Store::open(&paths.db)
            .unwrap()
            .get(&gone)
            .unwrap()
            .is_some()
    );

    config(
        &paths,
        &format!("roots = [{:?}]", other.display().to_string()),
    );
    service(&paths, &notifier, &clock, ScriptedEvents::default())
        .ensure_scanned()
        .unwrap();
    let store = Store::open(&paths.db).unwrap();
    assert!(store.get(&other).unwrap().is_some(), "added root scanned");
    assert!(store.get(&gone).unwrap().is_none(), "dropped root removed");
}

#[test]
fn reconcile_drops_newly_excluded_folders() {
    let (_d, paths) = home();
    write_old(&paths.home.join("keep/x.bin"), 300_000);
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    let config_dir = paths.config.parent().unwrap().to_path_buf();
    fs::create_dir_all(&config_dir).unwrap();
    let mut s = service(
        &paths,
        &notifier,
        &clock,
        ScriptedEvents::new(vec![change(&config_dir, 1)]),
    );
    s.ensure_scanned().unwrap();
    config(&paths, "excludes = [\"~/keep\"]");
    s.step(Duration::ZERO).unwrap();
    assert!(
        Store::open(&paths.db)
            .unwrap()
            .get(&paths.home.join("keep"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn reconcile_rereads_folders_that_were_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let (_d, paths) = home();
    let locked = paths.home.join("locked");
    write_old(&locked.join("x.bin"), 300_000);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let notifier = RecordingNotifier::default();
    let clock = || T0;
    service(&paths, &notifier, &clock, ScriptedEvents::default())
        .ensure_scanned()
        .unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    service(&paths, &notifier, &clock, ScriptedEvents::default())
        .ensure_scanned()
        .unwrap();
    let row = Store::open(&paths.db)
        .unwrap()
        .get(&locked)
        .unwrap()
        .unwrap();
    assert!(!row.flags.contains(attic_core::store::Flags::UNREADABLE));
    assert!(row.size > 0);
}

#[test]
fn idle_minutes_do_not_recompute() {
    let (_d, paths) = home();
    write_old(&paths.home.join("a/small.bin"), 150_000);
    config(
        &paths,
        "min_file_size = \"100kB\"\nnotify_threshold = \"1MB\"",
    );
    let notifier = RecordingNotifier::default();
    let now = Cell::new(T0);
    let clock = || now.get();
    let events = ScriptedEvents::new(vec![]);
    let mut s = service(&paths, &notifier, &clock, events);
    s.ensure_scanned().unwrap();
    s.step(Duration::ZERO).unwrap();
    // Grow the store behind the service's back without an event.
    let mut other = Store::open(&paths.db).unwrap();
    let mut big = other.get(&paths.home.join("a/small.bin")).unwrap().unwrap();
    big.path = paths.home.join("a/huge.bin");
    big.size = 9_000_000;
    other.transaction(|tx| tx.put_entry(&big)).unwrap();
    now.set(T0 + 600);
    s.step(Duration::ZERO).unwrap();
    assert!(notifier.sent().is_empty(), "no events, no recompute");
}
