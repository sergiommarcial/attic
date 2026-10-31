use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use attic_core::events::{EventBatch, EventSource};
use attic_core::fsevents::FsEventSource;

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

/// Waits up to 10 s for a batch mentioning `dir`; returns every batch seen.
fn wait_for(source: &mut FsEventSource, dir: &Path) -> Vec<EventBatch> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Some(batch) = source.next_batch(Duration::from_millis(500)).unwrap() {
            let hit = batch.changes.iter().any(|c| c.dir == dir);
            seen.push(batch);
            if hit {
                return seen;
            }
        }
    }
    panic!("no event for {} within 10 s; saw {seen:?}", dir.display());
}

#[test]
fn sees_a_new_file() {
    let (_d, root) = temp();
    fs::create_dir(root.join("sub")).unwrap();
    let mut source = FsEventSource::start(
        std::slice::from_ref(&root),
        None,
        Duration::from_millis(100),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    fs::write(root.join("sub/new.txt"), b"hi").unwrap();
    let batches = wait_for(&mut source, &root.join("sub"));
    assert!(batches.iter().all(|b| b.last_id > 0));
    for c in batches.iter().flat_map(|b| &b.changes) {
        assert!(
            !c.dir.as_os_str().as_encoded_bytes().ends_with(b"/"),
            "{:?}",
            c.dir
        );
    }
}

#[test]
fn replays_since_an_id() {
    let (_d, root) = temp();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("later")).unwrap();
    let mut source = FsEventSource::start(
        std::slice::from_ref(&root),
        None,
        Duration::from_millis(100),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    fs::write(root.join("first/a.txt"), b"a").unwrap();
    let last_id = wait_for(&mut source, &root.join("first"))
        .last()
        .unwrap()
        .last_id;
    drop(source);

    fs::write(root.join("later/b.txt"), b"b").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut again = FsEventSource::start(
        std::slice::from_ref(&root),
        Some(last_id),
        Duration::from_millis(100),
    )
    .unwrap();
    wait_for(&mut again, &root.join("later"));
}

#[test]
fn timeout_returns_none() {
    let (_d, root) = temp();
    let mut source = FsEventSource::start(
        std::slice::from_ref(&root),
        None,
        Duration::from_millis(100),
    )
    .unwrap();
    assert_eq!(source.next_batch(Duration::from_millis(300)).unwrap(), None);
}

#[test]
fn a_root_that_is_not_utf8_is_an_error_not_a_crash() {
    use std::os::unix::ffi::OsStrExt;
    let root = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9"));
    assert!(FsEventSource::start(&[root], None, Duration::from_millis(100)).is_err());
}

#[test]
fn dropping_a_source_returns_promptly() {
    let (_d, root) = temp();
    let source = FsEventSource::start(
        std::slice::from_ref(&root),
        None,
        Duration::from_millis(100),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    drop(source);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}
