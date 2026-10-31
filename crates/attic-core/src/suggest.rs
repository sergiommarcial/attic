//! What to suggest cleaning, and how much that would free.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::probe::Probe;
use crate::store::{Entry, Flags, Kind};

/// How many suggestions the terminal app shows, and the notification total
/// is summed over.
pub const LIST_LEN: usize = 20;
/// A folder whose largest child holds this share of it defers to the child.
pub const CONCENTRATION: f64 = 0.8;
/// Used within this many days counts as recent.
pub const RECENT_DAYS: u64 = 7;
/// At most this many suggestions are listed under another.
pub const MAX_NESTED: usize = 5;

const STANDARD_HOME_FOLDERS: [&str; 7] = [
    "Documents",
    "Desktop",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
];
const REBUILDABLE_NAMES: [&str; 3] = ["node_modules", ".venv", "__pycache__"];

/// Why a suggestion needs a second confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Warning {
    /// Holds a git repository with uncommitted or unpushed work.
    UnsavedRepo,
    /// A macOS library (Photos, Mail, Messages) or something inside one.
    LibraryBundle,
    /// Used in the last week.
    RecentlyUsed,
}

/// One thing that could be cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// What to clean.
    pub path: PathBuf,
    /// Folder or file.
    pub kind: Kind,
    /// Bytes it takes.
    pub size: u64,
    /// Days since last used (or modified, when Spotlight doesn't know).
    pub idle_days: u64,
    /// Rebuilt by its tool when needed (build output, caches).
    pub rebuildable: bool,
    /// See [`Warning`].
    pub warnings: Vec<Warning>,
    /// Lesser suggestions inside this one, already counted in `size`.
    pub nested: Vec<Suggestion>,
}

/// The suggestions, best first, and what they would free together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suggestions {
    /// Top-level suggestions, best first.
    pub items: Vec<Suggestion>,
    /// The sum of the top-level sizes (nested ones are inside them).
    pub total: u64,
}

/// Ranks `entries` by size weighted by time since last used and returns
/// the best `limit`, with lesser ones inside them nested and folders that
/// contain a better pick left out. Roots, `home` and
/// the standard home folders are never suggested whole. `probe` is asked
/// only about the suggestions returned.
pub fn suggest(
    entries: &[Entry],
    home: &Path,
    roots: &[PathBuf],
    now: i64,
    probe: &dyn Probe,
    limit: usize,
) -> Suggestions {
    let mut largest_child: HashMap<&Path, u64> = HashMap::new();
    let mut holds_candidates: HashSet<&Path> = HashSet::new();
    for e in entries {
        if let Some(parent) = e.path.parent() {
            let slot = largest_child.entry(parent).or_default();
            *slot = (*slot).max(e.size);
        }
        holds_candidates.extend(e.path.ancestors().skip(1));
    }

    let mut ranked: Vec<(f64, &Entry)> = entries
        .iter()
        .filter(|e| !e.flags.contains(Flags::UNREADABLE) && !is_protected(&e.path, home, roots))
        .filter(|e| {
            let child = largest_child.get(e.path.as_path());
            e.kind == Kind::File
                || (child.is_none_or(|&c| (c as f64) < CONCENTRATION * e.size as f64)
                    // A folder in use defers to whatever old is inside it.
                    && (!holds_candidates.contains(e.path.as_path())
                        || idle_days(e, now) >= RECENT_DAYS))
        })
        .map(|e| (score(e.size, idle_days(e, now)), e))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));

    let mut chosen: Vec<Suggestion> = Vec::new();
    for (_, e) in ranked {
        if let Some(parent) = chosen.iter_mut().find(|c| e.path.starts_with(&c.path)) {
            if parent.nested.len() < MAX_NESTED {
                parent.nested.push(plain(e, now));
            }
        } else if chosen.len() < limit && !chosen.iter().any(|c| c.path.starts_with(&e.path)) {
            // An ancestor of something already chosen would only add the
            // parts that ranked lower; it is left out.
            chosen.push(plain(e, now));
        }
    }

    for item in &mut chosen {
        assess(item, home, now, probe);
    }
    let total = chosen.iter().map(|s| s.size).sum();
    Suggestions {
        items: chosen,
        total,
    }
}

/// Never suggested or cleaned whole: a scan root, the home folder and the
/// standard home folders (what is inside them is fair game).
pub fn is_protected(path: &Path, home: &Path, roots: &[PathBuf]) -> bool {
    path == home
        || roots.iter().any(|r| r == path)
        || STANDARD_HOME_FOLDERS.iter().any(|f| path == home.join(f))
}

fn score(size: u64, idle_days: u64) -> f64 {
    size as f64 * (1.0 + idle_days as f64 / 30.0)
}

fn idle_days(e: &Entry, now: i64) -> u64 {
    let used = e.last_used.unwrap_or(e.newest_mtime);
    (now - used).max(0) as u64 / 86_400
}

fn plain(e: &Entry, now: i64) -> Suggestion {
    Suggestion {
        path: e.path.clone(),
        kind: e.kind,
        size: e.size,
        idle_days: idle_days(e, now),
        rebuildable: e.flags.contains(Flags::CACHEDIR),
        warnings: Vec::new(),
        nested: Vec::new(),
    }
}

/// Asks `probe` about one suggestion and sets its `idle_days`, warnings and
/// `rebuildable`. [`suggest`] does this for the items it returns; nested
/// ones are left plain, so call this before acting on one of them.
pub fn assess(item: &mut Suggestion, home: &Path, now: i64, probe: &dyn Probe) {
    item.warnings.clear();
    // The newer of Spotlight's date and the newest change inside wins.
    if let Some(used) = probe.last_used(&item.path) {
        item.idle_days = item.idle_days.min((now - used).max(0) as u64 / 86_400);
    }
    if item.kind == Kind::Dir && probe.repo_has_unsaved_work(&item.path) {
        item.warnings.push(Warning::UnsavedRepo);
    }
    if is_library(&item.path, home) {
        item.warnings.push(Warning::LibraryBundle);
    }
    if item.idle_days < RECENT_DAYS {
        item.warnings.push(Warning::RecentlyUsed);
    }
    item.rebuildable |= item
        .path
        .file_name()
        .is_some_and(|n| REBUILDABLE_NAMES.iter().any(|r| n == OsStr::new(r)))
        || item.path.starts_with(home.join("Library/Caches"));
}

fn is_library(path: &Path, home: &Path) -> bool {
    path.ancestors()
        .any(|a| a.extension().is_some_and(|x| x == "photoslibrary"))
        || path.starts_with(home.join("Library/Mail"))
        || path.starts_with(home.join("Library/Messages"))
}
