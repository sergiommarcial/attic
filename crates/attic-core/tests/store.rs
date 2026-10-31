use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::thread;

use attic_core::AtticError;
use attic_core::store::{Entry, Flags, Kind, Store};

fn dir(path: &str, size: u64, files: u64) -> Entry {
    Entry {
        path: PathBuf::from(path),
        kind: Kind::Dir,
        size,
        files,
        small_files: 0,
        small_bytes: 0,
        newest_mtime: 100,
        last_used: None,
        flags: Flags::empty(),
    }
}

fn file(path: &str, size: u64) -> Entry {
    Entry {
        kind: Kind::File,
        files: 1,
        ..dir(path, size, 1)
    }
}

fn size_of(store: &Store, path: &str) -> Option<u64> {
    store.get(Path::new(path)).unwrap().map(|e| e.size)
}

fn seeded() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .replace_subtree(
            Path::new("/h"),
            &[dir("/h", 10, 1), dir("/h/a", 10, 1), file("/h/a/b", 10)],
        )
        .unwrap();
    store
}

#[test]
fn replace_subtree_updates_ancestors() {
    let mut store = seeded();
    store
        .replace_subtree(
            Path::new("/h/a"),
            &[dir("/h/a", 30, 2), file("/h/a/b", 20), file("/h/a/c", 10)],
        )
        .unwrap();
    assert_eq!(size_of(&store, "/h"), Some(30));
    assert_eq!(store.get(Path::new("/h")).unwrap().unwrap().files, 2);
    assert_eq!(size_of(&store, "/h/a/c"), Some(10));
}

#[test]
fn replace_subtree_drops_rows_no_longer_there() {
    let mut store = seeded();
    store
        .replace_subtree(Path::new("/h/a"), &[dir("/h/a", 0, 0)])
        .unwrap();
    assert_eq!(size_of(&store, "/h/a/b"), None);
    assert_eq!(size_of(&store, "/h"), Some(0));
}

#[test]
fn remove_subtree_subtracts_and_returns_bytes() {
    let mut store = seeded();
    assert_eq!(store.remove_subtree(Path::new("/h/a")).unwrap(), 10);
    assert_eq!(size_of(&store, "/h/a"), None);
    assert_eq!(size_of(&store, "/h/a/b"), None);
    assert_eq!(size_of(&store, "/h"), Some(0));
    assert_eq!(store.get(Path::new("/h")).unwrap().unwrap().files, 0);
}

#[test]
fn removing_a_missing_path_is_zero() {
    let mut store = seeded();
    assert_eq!(store.remove_subtree(Path::new("/h/nope")).unwrap(), 0);
    assert_eq!(size_of(&store, "/h"), Some(10));
}

#[test]
fn prefix_names_are_not_descendants() {
    let mut store = seeded();
    store
        .replace_subtree(Path::new("/h/ab"), &[dir("/h/ab", 5, 0)])
        .unwrap();
    store.remove_subtree(Path::new("/h/a")).unwrap();
    assert_eq!(size_of(&store, "/h/ab"), Some(5));
}

#[test]
fn odd_bytes_round_trip() {
    let odd = Path::new(OsStr::from_bytes(b"/h/caf\xe9 %_\nx"));
    let mut store = seeded();
    let mut entry = file("/x", 7);
    entry.path = odd.to_path_buf();
    store.replace_subtree(odd, &[entry.clone()]).unwrap();
    assert_eq!(store.get(odd).unwrap(), Some(entry));
    assert_eq!(size_of(&store, "/h"), Some(17));
    assert!(
        store
            .children(Path::new("/h"))
            .unwrap()
            .iter()
            .any(|e| e.path == odd)
    );
}

#[test]
fn children_sorted_by_size() {
    let mut store = seeded();
    store
        .replace_subtree(Path::new("/h/z"), &[dir("/h/z", 50, 0)])
        .unwrap();
    let names: Vec<_> = store
        .children(Path::new("/h"))
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(names, [PathBuf::from("/h/z"), PathBuf::from("/h/a")]);
}

#[test]
fn candidates_are_big_dirs_and_files() {
    let mut store = seeded();
    store
        .replace_subtree(Path::new("/h/big"), &[dir("/h/big", 900, 0)])
        .unwrap();
    let mut got: Vec<_> = store
        .candidates(100)
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    got.sort();
    // /h grew to 910 with /h/big below it, so it qualifies too; the
    // 10-byte file is below the minimum (left from a lower setting).
    assert_eq!(got, [PathBuf::from("/h"), PathBuf::from("/h/big")]);
}

#[test]
fn unreadable_listed() {
    let mut store = seeded();
    let mut locked = dir("/h/locked", 0, 0);
    locked.flags.insert(Flags::UNREADABLE);
    store
        .replace_subtree(Path::new("/h/locked"), &[locked])
        .unwrap();
    assert_eq!(store.unreadable().unwrap(), [PathBuf::from("/h/locked")]);
}

#[test]
fn meta_round_trips() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.meta("fsevents_last_id").unwrap(), None);
    store.set_meta("fsevents_last_id", "42").unwrap();
    store.set_meta("fsevents_last_id", "43").unwrap();
    assert_eq!(
        store.meta("fsevents_last_id").unwrap().as_deref(),
        Some("43")
    );
}

#[test]
fn transaction_reads_and_writes_together() {
    let mut store = seeded();
    store
        .transaction(|tx| {
            assert_eq!(tx.get(Path::new("/h/a"))?.map(|e| e.size), Some(10));
            assert_eq!(tx.children(Path::new("/h"))?.len(), 1);
            tx.remove_subtree(Path::new("/h/a"))?;
            tx.set_meta("fsevents_last_id", "7")
        })
        .unwrap();
    assert_eq!(size_of(&store, "/h/a"), None);
    assert_eq!(
        store.meta("fsevents_last_id").unwrap().as_deref(),
        Some("7")
    );
}

#[test]
fn opens_on_disk_and_reopens() {
    let dir_ = tempfile::tempdir().unwrap();
    let db = dir_.path().join("nested/attic.db");
    {
        let mut store = Store::open(&db).unwrap();
        store.set_meta("k", "v").unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(store.meta("k").unwrap().as_deref(), Some("v"));
}

#[test]
fn refuses_newer_schema() {
    let dir_ = tempfile::tempdir().unwrap();
    let db = dir_.path().join("attic.db");
    drop(Store::open(&db).unwrap());
    rusqlite::Connection::open(&db)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let err = Store::open(&db).unwrap_err();
    assert!(
        matches!(err, AtticError::DbTooNew { found: 99, .. }),
        "{err}"
    );
}

#[test]
fn two_writers_do_not_lock() {
    let dir_ = tempfile::tempdir().unwrap();
    let db = dir_.path().join("attic.db");
    drop(Store::open(&db).unwrap());
    let writers: Vec<_> = ["/h/one", "/h/two"]
        .into_iter()
        .map(|root| {
            let db = db.clone();
            thread::spawn(move || {
                let mut store = Store::open(&db).unwrap();
                for i in 0..200 {
                    store
                        .replace_subtree(Path::new(root), &[dir(root, i, 0)])
                        .unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(size_of(&store, "/h/one"), Some(199));
    assert_eq!(size_of(&store, "/h/two"), Some(199));
}

#[test]
fn tops_are_rows_without_a_parent_row() {
    let mut store = seeded();
    store
        .replace_subtree(Path::new("/other"), &[dir("/other", 1, 0)])
        .unwrap();
    let mut tops = store.tops().unwrap();
    tops.sort();
    assert_eq!(tops, [PathBuf::from("/h"), PathBuf::from("/other")]);
}

#[test]
fn first_open_from_two_processes_at_once() {
    for _ in 0..10 {
        let dir_ = tempfile::tempdir().unwrap();
        let db = dir_.path().join("attic.db");
        let openers: Vec<_> = (0..4)
            .map(|_| {
                let db = db.clone();
                thread::spawn(move || Store::open(&db).map(drop))
            })
            .collect();
        for o in openers {
            o.join().unwrap().unwrap();
        }
    }
}

#[test]
fn totals_are_not_clamped_so_drift_does_not_compound() {
    let mut store = Store::open_in_memory().unwrap();
    // A parent that (wrongly) holds less than its child.
    store
        .replace_subtree(Path::new("/h"), &[dir("/h", 5, 0), file("/h/c", 10)])
        .unwrap();
    store.remove_subtree(Path::new("/h/c")).unwrap();
    store
        .replace_subtree(Path::new("/h/c"), &[file("/h/c", 10)])
        .unwrap();
    assert_eq!(size_of(&store, "/h"), Some(5));
}

#[test]
fn wal_file_is_size_limited() {
    let dir_ = tempfile::tempdir().unwrap();
    let store = Store::open(&dir_.path().join("attic.db")).unwrap();
    assert_eq!(store.journal_size_limit().unwrap(), 64 * 1024 * 1024);
}
