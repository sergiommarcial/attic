use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use attic_core::Config;
use attic_core::scan::{full_scan, scan_subtree};
use attic_core::store::{Entry, Flags, Kind, Store};

const MB: usize = 1_000_000;

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = File::create(path).unwrap();
    f.write_all(&vec![7u8; bytes]).unwrap();
}

fn blocks(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().blocks() * 512
}

fn cfg(min: u64) -> Config {
    Config {
        min_file_size: min,
        ..Config::default()
    }
}

fn find<'a>(entries: &'a [Entry], path: &Path) -> Option<&'a Entry> {
    entries.iter().find(|e| e.path == path)
}

/// root/a.bin (1 MB) and root/sub/b.bin (3 MB).
fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("root");
    write(&root.join("a.bin"), MB);
    write(&root.join("sub/b.bin"), 3 * MB);
    (dir, root)
}

#[test]
fn sizes_are_allocated_and_summed() {
    let (_d, root) = tree();
    let entries = scan_subtree(&root, &cfg(0)).unwrap();
    let want = blocks(&root.join("a.bin")) + blocks(&root.join("sub/b.bin"));
    let top = find(&entries, &root).unwrap();
    assert_eq!(top.kind, Kind::Dir);
    assert_eq!(top.size, want);
    assert_eq!(top.files, 2);
    assert_eq!(
        find(&entries, &root.join("sub")).unwrap().size,
        blocks(&root.join("sub/b.bin"))
    );
}

#[test]
fn small_files_aggregate() {
    let (_d, root) = tree();
    let entries = scan_subtree(&root, &cfg(2 * MB as u64)).unwrap();
    assert!(find(&entries, &root.join("a.bin")).is_none());
    let top = find(&entries, &root).unwrap();
    assert_eq!(top.small_files, 1);
    assert_eq!(top.small_bytes, blocks(&root.join("a.bin")));
    let b = find(&entries, &root.join("sub/b.bin")).unwrap();
    assert_eq!(b.kind, Kind::File);
    assert_eq!(b.size, blocks(&root.join("sub/b.bin")));
    assert_eq!(top.files, 2);
}

#[test]
fn newest_mtime_bubbles_up() {
    let (_d, root) = tree();
    let late = SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000);
    File::options()
        .write(true)
        .open(root.join("sub/b.bin"))
        .unwrap()
        .set_modified(late)
        .unwrap();
    let entries = scan_subtree(&root, &cfg(0)).unwrap();
    assert_eq!(find(&entries, &root).unwrap().newest_mtime, 4_000_000_000);
    assert_eq!(
        find(&entries, &root.join("sub")).unwrap().newest_mtime,
        4_000_000_000
    );
}

#[test]
fn cachedir_and_git_flags() {
    let (_d, root) = tree();
    write(&root.join("target/CACHEDIR.TAG"), 10);
    fs::create_dir_all(root.join("repo/.git")).unwrap();
    let entries = scan_subtree(&root, &cfg(0)).unwrap();
    assert!(
        find(&entries, &root.join("target"))
            .unwrap()
            .flags
            .contains(Flags::CACHEDIR)
    );
    assert!(
        find(&entries, &root.join("repo"))
            .unwrap()
            .flags
            .contains(Flags::GIT)
    );
    assert!(!find(&entries, &root).unwrap().flags.contains(Flags::GIT));
}

#[test]
fn symlinks_not_followed() {
    let (d, root) = tree();
    let outside = d.path().join("outside.bin");
    write(&outside, 5 * MB);
    symlink(&outside, root.join("link.bin")).unwrap();
    symlink(&root, root.join("sub/loop")).unwrap();
    let entries = scan_subtree(&root, &cfg(0)).unwrap();
    let want = blocks(&root.join("a.bin")) + blocks(&root.join("sub/b.bin"));
    assert_eq!(find(&entries, &root).unwrap().size, want);
    assert!(find(&entries, &root.join("link.bin")).is_none());
    assert!(find(&entries, &root.join("sub/loop")).is_none());
}

#[test]
fn hard_link_counted_once() {
    let (_d, root) = tree();
    fs::hard_link(root.join("sub/b.bin"), root.join("b-again.bin")).unwrap();
    let entries = scan_subtree(&root, &cfg(0)).unwrap();
    let want = blocks(&root.join("a.bin")) + blocks(&root.join("sub/b.bin"));
    assert_eq!(find(&entries, &root).unwrap().size, want);
}

#[test]
fn unreadable_dir_flagged() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let (_d, root) = tree();
    let locked = root.join("locked");
    write(&locked.join("secret.bin"), MB);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let result = scan_subtree(&root, &cfg(0));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let entries = result.unwrap();
    let e = find(&entries, &locked).unwrap();
    assert!(e.flags.contains(Flags::UNREADABLE));
    assert_eq!(e.size, 0);
}

#[test]
fn odd_names_scan() {
    let (_d, root) = tree();
    let odd = root.join("we\nird %_ café 🗄").join("big file.bin");
    write(&odd, 3 * MB);
    let entries = scan_subtree(&root, &cfg(2 * MB as u64)).unwrap();
    assert_eq!(find(&entries, &odd).unwrap().size, blocks(&odd));
}

#[test]
fn excludes_are_skipped() {
    let (_d, root) = tree();
    let config = Config {
        excludes: vec![root.join("sub")],
        ..cfg(0)
    };
    let entries = scan_subtree(&root, &config).unwrap();
    assert!(find(&entries, &root.join("sub")).is_none());
    assert_eq!(
        find(&entries, &root).unwrap().size,
        blocks(&root.join("a.bin"))
    );
}

#[test]
fn missing_root_is_empty() {
    let (_d, root) = tree();
    assert!(
        scan_subtree(&root.join("gone"), &cfg(0))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn vanished_entries_skipped() {
    let (_d, root) = tree();
    let churn = root.join("churn");
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let (churn, stop) = (churn.clone(), stop.clone());
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for i in 0..20 {
                    write(&churn.join(format!("d{i}/f.bin")), 1000);
                }
                let _ = fs::remove_dir_all(&churn);
            }
        })
    };
    for _ in 0..30 {
        scan_subtree(&root, &cfg(0)).unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}

#[test]
fn full_scan_stores_rows_and_reports_progress() {
    let (_d, root) = tree();
    let mut store = Store::open_in_memory().unwrap();
    let mut calls = 0;
    full_scan(
        &mut store,
        std::slice::from_ref(&root),
        &cfg(0),
        &mut |_| calls += 1,
    )
    .unwrap();
    let want = blocks(&root.join("a.bin")) + blocks(&root.join("sub/b.bin"));
    assert_eq!(store.get(&root).unwrap().unwrap().size, want);
    assert!(store.meta("full_scan_done_at").unwrap().is_some());
    assert!(calls >= 1, "progress is reported at least once per root");
}
