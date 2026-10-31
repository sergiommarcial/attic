use std::cell::Cell;
use std::path::{Path, PathBuf};

use attic_core::probe::{FakeProbe, Probe};
use attic_core::store::{Entry, Flags, Kind};
use attic_core::suggest::{Warning, suggest};

const DAY: i64 = 86_400;
const NOW: i64 = 1_000 * DAY;
const GB: u64 = 1_000_000_000;

fn entry(path: &str, kind: Kind, size: u64, idle_days: i64) -> Entry {
    Entry {
        path: PathBuf::from(path),
        kind,
        size,
        files: 1,
        small_files: 0,
        small_bytes: 0,
        newest_mtime: NOW - idle_days * DAY,
        last_used: None,
        flags: Flags::empty(),
    }
}

fn dir(path: &str, size: u64, idle: i64) -> Entry {
    entry(path, Kind::Dir, size, idle)
}

fn file(path: &str, size: u64, idle: i64) -> Entry {
    entry(path, Kind::File, size, idle)
}

fn run(entries: &[Entry], probe: &dyn Probe, limit: usize) -> attic_core::suggest::Suggestions {
    suggest(
        entries,
        Path::new("/h"),
        &[PathBuf::from("/h")],
        NOW,
        probe,
        limit,
    )
}

fn paths(s: &attic_core::suggest::Suggestions) -> Vec<&Path> {
    s.items.iter().map(|i| i.path.as_path()).collect()
}

#[test]
fn big_and_old_ranks_first() {
    let entries = [dir("/h/fresh", 30 * GB, 0), dir("/h/old", 20 * GB, 240)];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s), [Path::new("/h/old"), Path::new("/h/fresh")]);
    assert_eq!(s.items[0].idle_days, 240);
}

#[test]
fn protected_paths_never_suggested() {
    let entries = [
        dir("/h", 900 * GB, 400),
        dir("/h/Documents", 400 * GB, 400),
        dir("/h/Library", 300 * GB, 400),
        dir("/h/Documents/old", 5 * GB, 400),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s), [Path::new("/h/Documents/old")]);
}

#[test]
fn concentrated_dir_defers_to_child() {
    let entries = [
        dir("/h/proj", 10 * GB, 100),
        dir("/h/proj/target", 9 * GB, 100),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s), [Path::new("/h/proj/target")]);
}

#[test]
fn nested_not_double_counted() {
    let entries = [
        dir("/h/old", 10 * GB, 300),
        dir("/h/old/a", 4 * GB, 300),
        file("/h/old/b.mov", 3 * GB, 300),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s), [Path::new("/h/old")]);
    assert_eq!(s.items[0].nested.len(), 2);
    assert_eq!(s.total, 10 * GB);
}

#[test]
fn ancestor_of_a_chosen_item_is_skipped() {
    // The child is older, so it ranks first; its parent must not swallow it.
    let entries = [
        dir("/h/p", 10 * GB, 10),
        dir("/h/p/c", 6 * GB, 600),
        dir("/h/p/d", 3 * GB, 10),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert!(!paths(&s).contains(&Path::new("/h/p")), "{:?}", paths(&s));
    assert_eq!(paths(&s)[0], Path::new("/h/p/c"));
    assert_eq!(s.total, s.items.iter().map(|i| i.size).sum::<u64>());
}

#[test]
fn old_build_output_beats_the_projects_folder() {
    let entries = [
        dir("/h/repos", 80 * GB, 0),
        dir("/h/repos/old/target", 38 * GB, 240),
        dir("/h/repos/cur", 42 * GB, 0),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s)[0], Path::new("/h/repos/old/target"));
    assert!(
        !paths(&s).contains(&Path::new("/h/repos")),
        "{:?}",
        paths(&s)
    );
}

#[test]
fn recently_used_folder_defers_to_older_children() {
    let entries = [
        dir("/h/repos", 500 * GB, 0),
        dir("/h/repos/old/target", 38 * GB, 240),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(paths(&s), [Path::new("/h/repos/old/target")]);
}

#[test]
fn newer_change_beats_an_older_spotlight_date() {
    let mut probe = FakeProbe::default();
    probe
        .last_used
        .insert(PathBuf::from("/h/p"), NOW - 300 * DAY);
    let s = run(&[dir("/h/p", 5 * GB, 1)], &probe, 20);
    assert_eq!(s.items[0].idle_days, 1);
    assert_eq!(s.items[0].warnings, [Warning::RecentlyUsed]);
}

#[test]
fn protected_paths_are_reported() {
    use attic_core::suggest::is_protected;
    let roots = [PathBuf::from("/h"), PathBuf::from("/data")];
    for p in ["/h", "/data", "/h/Music", "/h/Library"] {
        assert!(is_protected(Path::new(p), Path::new("/h"), &roots), "{p}");
    }
    assert!(!is_protected(
        Path::new("/h/Music/old"),
        Path::new("/h"),
        &roots
    ));
}

#[test]
fn warnings_set() {
    let mut probe = FakeProbe::default();
    probe.unsaved.insert(PathBuf::from("/h/code"));
    let entries = [
        dir("/h/code", 5 * GB, 100),
        dir("/h/Pictures/Photos Library.photoslibrary", 50 * GB, 100),
        dir("/h/Library/Mail/V10", 3 * GB, 100),
        file("/h/Movies/new.mov", 2 * GB, 2),
    ];
    let s = run(&entries, &probe, 20);
    let warnings = |p: &str| {
        s.items
            .iter()
            .find(|i| i.path == Path::new(p))
            .unwrap()
            .warnings
            .clone()
    };
    assert_eq!(warnings("/h/code"), [Warning::UnsavedRepo]);
    assert_eq!(
        warnings("/h/Pictures/Photos Library.photoslibrary"),
        [Warning::LibraryBundle]
    );
    assert_eq!(warnings("/h/Library/Mail/V10"), [Warning::LibraryBundle]);
    assert_eq!(warnings("/h/Movies/new.mov"), [Warning::RecentlyUsed]);
}

#[test]
fn probe_last_used_refreshes_idle_days() {
    let mut probe = FakeProbe::default();
    probe
        .last_used
        .insert(PathBuf::from("/h/old.mov"), NOW - DAY);
    let s = run(&[file("/h/old.mov", 2 * GB, 500)], &probe, 20);
    assert_eq!(s.items[0].idle_days, 1);
    assert_eq!(s.items[0].warnings, [Warning::RecentlyUsed]);
}

#[test]
fn rebuildable_set() {
    let mut target = dir("/h/p/target", 5 * GB, 100);
    target.flags.insert(Flags::CACHEDIR);
    let entries = [
        target,
        dir("/h/q/node_modules", 2 * GB, 100),
        dir("/h/Library/Caches/com.app", 2 * GB, 100),
        dir("/h/plain", 2 * GB, 100),
    ];
    let s = run(&entries, &FakeProbe::default(), 20);
    let rebuildable: Vec<_> = s
        .items
        .iter()
        .filter(|i| i.rebuildable)
        .map(|i| i.path.as_path())
        .collect();
    assert_eq!(rebuildable.len(), 3);
    assert!(!rebuildable.contains(&Path::new("/h/plain")));
}

struct Counting(Cell<usize>);

impl Probe for Counting {
    fn last_used(&self, _: &Path) -> Option<i64> {
        self.0.set(self.0.get() + 1);
        None
    }
    fn repo_has_unsaved_work(&self, _: &Path) -> bool {
        false
    }
}

#[test]
fn probe_called_only_for_shown() {
    let entries: Vec<Entry> = (0..100)
        .map(|i| dir(&format!("/h/d{i:03}"), GB, 100))
        .collect();
    let probe = Counting(Cell::new(0));
    let s = run(&entries, &probe, 5);
    assert_eq!(s.items.len(), 5);
    assert_eq!(probe.0.get(), 5);
}

#[test]
fn limit_respected_and_unreadable_skipped() {
    let mut locked = dir("/h/locked", 99 * GB, 999);
    locked.flags.insert(Flags::UNREADABLE);
    let mut entries: Vec<Entry> = (0..30)
        .map(|i| dir(&format!("/h/d{i:02}"), GB, 100))
        .collect();
    entries.push(locked);
    let s = run(&entries, &FakeProbe::default(), 20);
    assert_eq!(s.items.len(), 20);
    assert!(!paths(&s).contains(&Path::new("/h/locked")));
}

#[test]
fn assess_warns_about_a_nested_item() {
    let mut probe = FakeProbe::default();
    probe.unsaved.insert(PathBuf::from("/h/old/repo"));
    let entries = [dir("/h/old", 10 * GB, 300), dir("/h/old/repo", 4 * GB, 300)];
    let s = run(&entries, &probe, 20);
    let mut nested = s.items[0].nested[0].clone();
    assert!(nested.warnings.is_empty());
    attic_core::suggest::assess(&mut nested, Path::new("/h"), NOW, &probe);
    assert_eq!(nested.warnings, [Warning::UnsavedRepo]);
}
