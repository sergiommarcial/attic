use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use attic_core::Config;
use attic_core::apply::apply_batch;
use attic_core::events::{Change, EventBatch};
use attic_core::scan::full_scan;
use attic_core::store::{Entry, Store};

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    File::create(path)
        .unwrap()
        .write_all(&vec![1u8; bytes])
        .unwrap();
}

fn cfg() -> Config {
    Config {
        min_file_size: 50_000,
        ..Config::default()
    }
}

/// home/{docs/a.txt 10k, docs/deep/b.bin 100k, big.bin 200k}
fn setup() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap().join("home");
    write(&home.join("docs/a.txt"), 10_000);
    write(&home.join("docs/deep/b.bin"), 100_000);
    write(&home.join("big.bin"), 200_000);
    let mut store = Store::open_in_memory().unwrap();
    full_scan(&mut store, std::slice::from_ref(&home), &cfg(), &mut |_| {}).unwrap();
    (dir, home, store)
}

fn batch(changes: &[(&Path, bool)]) -> EventBatch {
    EventBatch {
        changes: changes
            .iter()
            .map(|(dir, subtree)| Change {
                dir: dir.to_path_buf(),
                subtree: *subtree,
            })
            .collect(),
        last_id: 42,
        history_lost: false,
    }
}

fn rows(store: &Store) -> Vec<Entry> {
    let mut rows = store.all_entries().unwrap();
    for r in &mut rows {
        r.last_used = None;
    }
    rows
}

/// The store after applying events must equal a fresh scan of the disk.
fn assert_matches_disk(store: &Store, home: &Path) {
    let mut fresh = Store::open_in_memory().unwrap();
    full_scan(&mut fresh, &[home.to_path_buf()], &cfg(), &mut |_| {}).unwrap();
    assert_eq!(rows(store), rows(&fresh));
}

fn apply(store: &mut Store, home: &Path, b: &EventBatch) {
    apply_batch(store, &[home.to_path_buf()], b, &cfg()).unwrap();
}

#[test]
fn new_file_grows_dir_and_ancestors() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/deep/c.bin"), 300_000);
    apply(
        &mut store,
        &home,
        &batch(&[(&home.join("docs/deep"), false)]),
    );
    assert_matches_disk(&store, &home);
}

#[test]
fn small_file_change_updates_aggregates() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/more.txt"), 20_000);
    fs::remove_file(home.join("docs/a.txt")).unwrap();
    apply(&mut store, &home, &batch(&[(&home.join("docs"), false)]));
    assert_matches_disk(&store, &home);
}

#[test]
fn deleted_folder_removed_with_subtree() {
    let (_d, home, mut store) = setup();
    fs::remove_dir_all(home.join("docs/deep")).unwrap();
    apply(&mut store, &home, &batch(&[(&home.join("docs"), false)]));
    assert_matches_disk(&store, &home);
    assert!(store.get(&home.join("docs/deep/b.bin")).unwrap().is_none());
}

#[test]
fn new_folder_scanned_fully() {
    let (_d, home, mut store) = setup();
    write(&home.join("new/one/two/x.bin"), 400_000);
    apply(&mut store, &home, &batch(&[(&home, false)]));
    assert_matches_disk(&store, &home);
}

#[test]
fn events_deep_and_shallow_in_one_batch() {
    let (_d, home, mut store) = setup();
    write(&home.join("new/one/x.bin"), 400_000);
    write(&home.join("docs/deep/y.bin"), 60_000);
    apply(
        &mut store,
        &home,
        &batch(&[
            (&home, false),
            (&home.join("new/one"), false),
            (&home.join("new"), false),
            (&home.join("docs/deep"), false),
        ]),
    );
    assert_matches_disk(&store, &home);
}

#[test]
fn subtree_flag_rescans_below() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/deep/deeper/z.bin"), 70_000);
    apply(&mut store, &home, &batch(&[(&home.join("docs"), true)]));
    assert_matches_disk(&store, &home);
}

#[test]
fn history_lost_rescans_roots() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/deep/deeper/z.bin"), 70_000);
    fs::remove_file(home.join("big.bin")).unwrap();
    let mut b = batch(&[]);
    b.history_lost = true;
    apply(&mut store, &home, &b);
    assert_matches_disk(&store, &home);
}

#[test]
fn vanished_dir_removed() {
    let (_d, home, mut store) = setup();
    fs::remove_dir_all(home.join("docs")).unwrap();
    apply(
        &mut store,
        &home,
        &batch(&[
            (&home.join("docs/deep"), false),
            (&home.join("docs"), false),
        ]),
    );
    assert!(store.get(&home.join("docs")).unwrap().is_none());
    assert_matches_disk(&store, &home);
}

#[test]
fn last_id_persisted_with_changes() {
    let (_d, home, mut store) = setup();
    apply(&mut store, &home, &batch(&[(&home.join("docs"), false)]));
    assert_eq!(
        store.meta("fsevents_last_id").unwrap().as_deref(),
        Some("42")
    );
}

#[test]
fn outside_roots_ignored() {
    let (d, home, mut store) = setup();
    let elsewhere = d.path().join("elsewhere");
    write(&elsewhere.join("f.bin"), 90_000);
    apply(&mut store, &home, &batch(&[(&elsewhere, false)]));
    assert!(store.get(&elsewhere).unwrap().is_none());
    assert_matches_disk(&store, &home);
}

#[test]
fn file_path_event_treated_as_its_folder() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/deep/b.bin"), 150_000);
    apply(
        &mut store,
        &home,
        &batch(&[(&home.join("docs/deep/b.bin"), false)]),
    );
    assert_matches_disk(&store, &home);
}

#[test]
fn subtree_and_shallow_events_for_one_folder_rescan_below() {
    let (_d, home, mut store) = setup();
    write(&home.join("docs/deep/c.bin"), 300_000);
    let docs = home.join("docs");
    apply(&mut store, &home, &batch(&[(&docs, false), (&docs, true)]));
    assert_matches_disk(&store, &home);
}

#[test]
fn hard_links_count_the_same_in_shallow_and_full_passes() {
    let (_d, home, mut store) = setup();
    fs::hard_link(home.join("big.bin"), home.join("docs/big-again.bin")).unwrap();
    write(&home.join("docs/note.txt"), 1_000);
    apply(
        &mut store,
        &home,
        &batch(&[(&home, false), (&home.join("docs"), false)]),
    );
    assert_matches_disk(&store, &home);
}
