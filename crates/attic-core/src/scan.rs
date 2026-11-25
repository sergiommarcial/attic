//! Walking a folder tree into [`Entry`] rows.

use std::collections::HashMap;
use std::fs::Metadata;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jwalk::WalkDirGeneric;

use crate::Config;
use crate::Result;
use crate::store::{Entry, Flags, Kind, Store};

/// Folders macOS owns. They are never walked into, whatever the roots say.
pub const SYSTEM_PATHS: [&str; 7] = [
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/private/var",
    "/dev",
    "/Volumes",
];

const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How far a scan has got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanProgress {
    /// Folders seen so far.
    pub dirs: u64,
    /// Bytes counted so far.
    pub bytes: u64,
    /// The folder being read.
    pub current: PathBuf,
}

#[derive(Default)]
struct Dir {
    size: u64,
    files: u64,
    small_files: u64,
    small_bytes: u64,
    newest_mtime: i64,
    flags: Flags,
}

/// What the scan needs from `lstat`. It is read on the walker's worker
/// threads, so the consuming loop never waits on a syscall per entry.
#[derive(Debug, Clone, Copy)]
struct Stat {
    is_dir: bool,
    size: u64,
    mtime: i64,
}

fn stat(path: &Path) -> Option<Stat> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Stat {
        is_dir: meta.is_dir(),
        size: file_size(&meta),
        mtime: meta.mtime(),
    })
}

/// Walks `root` and returns one entry per folder and one per file of at
/// least `cfg.min_file_size`, with folder totals filled in. Symlinks are not
/// followed, a hard-linked file is split between its links, system folders and `cfg.excludes` are
/// skipped, an unreadable folder is flagged with size 0, and anything that
/// vanishes during the walk is left out. A missing `root` gives no entries.
///
/// # Errors
///
/// None today beyond what the walk cannot recover from; unreadable and
/// vanished entries are not errors.
pub fn scan_subtree(root: &Path, cfg: &Config) -> Result<Vec<Entry>> {
    scan_with_progress(root, cfg, &mut |_| {})
}

/// [`scan_subtree`], calling `progress` at most every 250 ms and once at
/// the end.
///
/// # Errors
///
/// As [`scan_subtree`].
pub fn scan_with_progress(
    root: &Path,
    cfg: &Config,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<Vec<Entry>> {
    if skipped(root, cfg) || std::fs::symlink_metadata(root).is_err() {
        return Ok(Vec::new());
    }
    let excludes = cfg.excludes.clone();
    let walk = WalkDirGeneric::<((), Option<Stat>)>::new(root)
        .follow_links(false)
        .skip_hidden(false)
        .sort(false)
        .process_read_dir(move |_, _, _, children| {
            children.retain(|child| match child {
                Ok(e) => !skipped_by(&e.parent_path.join(&e.file_name), &excludes),
                Err(_) => true,
            });
            for child in children.iter_mut().flatten() {
                if !child.file_type.is_symlink() {
                    child.client_state = stat(&child.parent_path.join(&child.file_name));
                }
            }
        });

    let mut dirs: HashMap<PathBuf, Dir> = HashMap::new();
    let mut files = Vec::new();
    let mut state = ScanProgress::default();
    let mut last_report = Instant::now();

    for item in walk {
        let entry = match item {
            Ok(entry) => entry,
            // A folder that cannot be read reports through its own entry
            // (`read_children`); an error here is a single child that
            // vanished or could not be listed, which is skipped.
            Err(_) => continue,
        };
        if entry.file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        // The root is nobody's child, so it is the one entry read here.
        let Some(meta) = entry
            .client_state
            .or_else(|| if entry.depth == 0 { stat(&path) } else { None })
        else {
            continue;
        };
        let parent = path.parent().map(Path::to_path_buf);
        let name = entry.file_name.as_os_str();

        if meta.is_dir {
            if name == ".git"
                && let Some(p) = parent.as_ref().and_then(|p| dirs.get_mut(p))
            {
                p.flags.insert(Flags::GIT);
            }
            state.dirs += 1;
            state.current = path.clone();
            let mut dir = Dir {
                newest_mtime: meta.mtime,
                ..Dir::default()
            };
            if let Some(err) = entry.read_children.as_ref().and_then(|r| r.error())
                && err
                    .io_error()
                    .is_none_or(|io| io.kind() != io::ErrorKind::NotFound)
            {
                dir.flags.insert(Flags::UNREADABLE);
            }
            dirs.insert(path, dir);
        } else {
            let size = meta.size;
            state.bytes += size;
            if let Some(p) = parent.as_ref().and_then(|p| dirs.get_mut(p)) {
                p.files += 1;
                p.size += size;
                p.newest_mtime = p.newest_mtime.max(meta.mtime);
                if name == "CACHEDIR.TAG" {
                    p.flags.insert(Flags::CACHEDIR);
                }
                if size < cfg.min_file_size {
                    p.small_files += 1;
                    p.small_bytes += size;
                }
            }
            if size >= cfg.min_file_size {
                files.push(Entry {
                    path,
                    kind: Kind::File,
                    size,
                    files: 1,
                    small_files: 0,
                    small_bytes: 0,
                    newest_mtime: meta.mtime,
                    last_used: None,
                    flags: Flags::empty(),
                });
            }
        }
        if last_report.elapsed() >= PROGRESS_EVERY {
            progress(&state);
            last_report = Instant::now();
        }
    }
    progress(&state);

    // An unreadable folder's size is unknown, not whatever was counted
    // before the read failed.
    for dir in dirs.values_mut() {
        if dir.flags.contains(Flags::UNREADABLE) {
            *dir = Dir {
                flags: dir.flags,
                newest_mtime: dir.newest_mtime,
                ..Dir::default()
            };
        }
    }
    roll_up(&mut dirs, root);

    let mut entries: Vec<Entry> = dirs
        .into_iter()
        .map(|(path, d)| Entry {
            path,
            kind: Kind::Dir,
            size: d.size,
            files: d.files,
            small_files: d.small_files,
            small_bytes: d.small_bytes,
            newest_mtime: d.newest_mtime,
            last_used: None,
            flags: d.flags,
        })
        .collect();
    entries.extend(files);
    Ok(entries)
}

/// Adds every folder's totals into its parent's, deepest first, so each
/// folder is folded once: O(n log n) for the sort, O(n) after.
fn roll_up(dirs: &mut HashMap<PathBuf, Dir>, root: &Path) {
    let mut order: Vec<PathBuf> = dirs.keys().filter(|p| *p != root).cloned().collect();
    order.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for path in order {
        let Some(child) = dirs.get(&path) else {
            continue;
        };
        let (size, files, mtime) = (child.size, child.files, child.newest_mtime);
        if let Some(parent) = path.parent().and_then(|p| dirs.get_mut(p)) {
            parent.size += size;
            parent.files += files;
            parent.newest_mtime = parent.newest_mtime.max(mtime);
        }
    }
}

/// Scans every root into the store, replacing what it held for them, then
/// records `full_scan_done_at`. Progress also goes to the `scan_progress`
/// meta key, as `dirs bytes current`, so the terminal app can show it.
///
/// # Errors
///
/// [`crate::AtticError::Db`] when the store fails.
pub fn full_scan(
    store: &mut Store,
    roots: &[PathBuf],
    cfg: &Config,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<()> {
    for root in roots {
        let mut failed = Ok(());
        let entries = scan_with_progress(root, cfg, &mut |p| {
            progress(p);
            let line = format!("{} {} {}", p.dirs, p.bytes, p.current.display());
            if let Err(e) = store.set_meta("scan_progress", &line) {
                failed = Err(e);
            }
        })?;
        failed?;
        store.replace_subtree(root, &entries)?;
    }
    store.set_meta("scan_progress", "")?;
    store.set_meta("full_scan_done_at", &now().to_string())
}

/// Seconds since the epoch, now.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn allocated(meta: &Metadata) -> u64 {
    meta.blocks() * 512
}

/// A file's share of the disk: its allocated size, split evenly between its
/// hard links, so every link's folder carries part of it and the parts add up
/// to the file once. A full scan and a one-folder rescan count it the same
/// way, whatever order they meet the links in.
pub(crate) fn file_size(meta: &Metadata) -> u64 {
    allocated(meta) / meta.nlink().max(1)
}

fn skipped(path: &Path, cfg: &Config) -> bool {
    skipped_by(path, &cfg.excludes)
}

pub(crate) fn skipped_by(path: &Path, excludes: &[PathBuf]) -> bool {
    SYSTEM_PATHS.iter().any(|s| path == Path::new(s)) || excludes.iter().any(|e| path == e)
}
