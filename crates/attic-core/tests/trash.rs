use std::fs;
use std::path::{Path, PathBuf};

use attic_core::AtticError;
use attic_core::Config;
use attic_core::scan::full_scan;
use attic_core::store::Store;
use attic_core::trash::{RecordingTrasher, TrashOutcome, Trasher, move_to_trash};

fn setup() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    fs::create_dir_all(home.join("old/inner")).unwrap();
    fs::write(home.join("old/inner/a.bin"), vec![1u8; 100_000]).unwrap();
    fs::write(home.join("big file\nodd.bin"), vec![1u8; 100_000]).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    let cfg = Config {
        min_file_size: 50_000,
        ..Config::default()
    };
    full_scan(&mut store, std::slice::from_ref(&home), &cfg, &mut |_| {}).unwrap();
    (dir, home, store)
}

fn trasher(dir: &tempfile::TempDir) -> RecordingTrasher {
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    RecordingTrasher::new(bin)
}

#[test]
fn moves_and_removes_rows() {
    let (d, home, mut store) = setup();
    let before = store.get(&home).unwrap().unwrap().size;
    let folder = store.get(&home.join("old")).unwrap().unwrap().size;
    let t = trasher(&d);
    let outcome = move_to_trash(&mut store, &t, &home.join("old")).unwrap();
    assert_eq!(outcome, TrashOutcome::Moved { bytes: folder });
    assert!(!home.join("old").exists());
    assert_eq!(t.moved(), [home.join("old")]);
    assert!(store.get(&home.join("old/inner/a.bin")).unwrap().is_none());
    assert_eq!(store.get(&home).unwrap().unwrap().size, before - folder);
}

#[test]
fn already_gone_removes_rows() {
    let (d, home, mut store) = setup();
    fs::remove_dir_all(home.join("old")).unwrap();
    let folder = store.get(&home.join("old")).unwrap().unwrap().size;
    let t = trasher(&d);
    let outcome = move_to_trash(&mut store, &t, &home.join("old")).unwrap();
    assert_eq!(outcome, TrashOutcome::AlreadyGone { bytes: folder });
    assert!(t.moved().is_empty());
    assert!(store.get(&home.join("old")).unwrap().is_none());
}

struct Failing;

impl Trasher for Failing {
    fn trash(&self, path: &Path) -> attic_core::Result<()> {
        Err(AtticError::Trash {
            path: path.to_path_buf(),
            message: "nope".into(),
        })
    }
}

#[test]
fn trash_failure_keeps_rows() {
    let (_d, home, mut store) = setup();
    let err = move_to_trash(&mut store, &Failing, &home.join("old")).unwrap_err();
    assert!(matches!(err, AtticError::Trash { .. }), "{err}");
    assert!(home.join("old").exists());
    assert!(store.get(&home.join("old")).unwrap().is_some());
}

#[test]
fn odd_name_trashed() {
    let (d, home, mut store) = setup();
    let odd = home.join("big file\nodd.bin");
    let t = trasher(&d);
    assert!(matches!(
        move_to_trash(&mut store, &t, &odd).unwrap(),
        TrashOutcome::Moved { .. }
    ));
    assert!(!odd.exists());
    assert!(store.get(&odd).unwrap().is_none());
}

#[test]
fn trashed_even_when_the_store_stays_busy() {
    let (d, home, _store) = setup();
    let db = d.path().join("attic.db");
    let mut store = Store::open(&db).unwrap();
    let cfg = Config {
        min_file_size: 50_000,
        ..Config::default()
    };
    full_scan(&mut store, std::slice::from_ref(&home), &cfg, &mut |_| {}).unwrap();
    let size = store.get(&home.join("old")).unwrap().unwrap().size;

    let blocker = rusqlite::Connection::open(&db).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(7));
        blocker.execute_batch("COMMIT").unwrap();
    });
    let t = trasher(&d);
    let outcome = move_to_trash(&mut store, &t, &home.join("old"));
    release.join().unwrap();
    assert_eq!(outcome.unwrap(), TrashOutcome::Moved { bytes: size });
    assert_eq!(t.moved(), [home.join("old")]);
}
