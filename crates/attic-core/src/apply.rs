//! Keeping the store in step with file system events.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::events::{EventBatch, LAST_EVENT_ID};
use crate::scan::{file_size, scan_subtree, skipped_by};
use crate::store::{Entry, Flags, Kind, Store, Tx};
use crate::{Config, Result};

/// Applies one batch: folders that vanished are removed, new folders and
/// `subtree` changes are scanned in full, other folders are re-read one
/// level deep, and the batch's `last_id` is recorded. A batch with
/// `history_lost` rescans every root. Paths outside `roots`, and excluded
/// or system paths, are ignored.
///
/// The disk is read first and the store written after, in one short
/// transaction, so a long scan never holds the write lock the terminal app
/// needs.
///
/// # Errors
///
/// [`crate::AtticError::Db`] when the store fails.
pub fn apply_batch(
    store: &mut Store,
    roots: &[PathBuf],
    batch: &EventBatch,
    cfg: &Config,
) -> Result<()> {
    let last_id = batch.last_id.to_string();
    let plans = if batch.history_lost {
        roots
            .iter()
            .map(|root| Ok(Plan::Replace(root.clone(), scan_subtree(root, cfg)?)))
            .collect::<Result<Vec<_>>>()?
    } else {
        plan(store, roots, batch, cfg)?
    };
    store.transaction(|tx| {
        for plan in &plans {
            plan.apply(tx)?;
        }
        tx.set_meta(LAST_EVENT_ID, &last_id)
    })
}

fn plan(store: &Store, roots: &[PathBuf], batch: &EventBatch, cfg: &Config) -> Result<Vec<Plan>> {
    let mut work = Vec::new();
    for (dir, subtree) in normalize(batch, roots, cfg) {
        work.push(classify(store, dir, subtree, roots)?);
    }
    let full: HashSet<PathBuf> = work
        .iter()
        .filter(|w| matches!(w, Work::Full(_)))
        .map(|w| w.path().to_path_buf())
        .collect();
    work.retain(|w| !w.path().ancestors().skip(1).any(|a| full.contains(a)));
    work.sort_by_key(|w| std::cmp::Reverse(w.path().components().count()));
    // Deepest first: a folder's one-level rescan reads its subfolders' rows
    // after their own changes are written.
    work.into_iter()
        .map(|w| match w {
            Work::Remove(path) => Ok(Plan::Remove(path)),
            Work::Full(path) => {
                let entries = scan_subtree(&path, cfg)?;
                Ok(Plan::Replace(path, entries))
            }
            Work::Shallow(path) => shallow(store, &path, cfg),
        })
        .collect()
}

enum Work {
    Remove(PathBuf),
    Full(PathBuf),
    Shallow(PathBuf),
}

impl Work {
    fn path(&self) -> &Path {
        match self {
            Work::Remove(p) | Work::Full(p) | Work::Shallow(p) => p,
        }
    }
}

/// A write worked out from the disk, applied later in the transaction.
enum Plan {
    Remove(PathBuf),
    Replace(PathBuf, Vec<Entry>),
    Shallow(ShallowPlan),
}

struct ShallowPlan {
    /// The folder's row with its direct files counted; its subfolders'
    /// totals are added when applied, from their rows as written by then.
    row: Entry,
    remove: Vec<PathBuf>,
    replace: Vec<(PathBuf, Vec<Entry>)>,
    put: Vec<Entry>,
    child_dirs: Vec<PathBuf>,
}

impl Plan {
    fn apply(&self, tx: &Tx) -> Result<()> {
        match self {
            Plan::Remove(path) => tx.remove_subtree(path).map(drop),
            Plan::Replace(path, entries) => tx.replace_subtree(path, entries),
            Plan::Shallow(s) => {
                for path in &s.remove {
                    tx.remove_subtree(path)?;
                }
                for (path, entries) in &s.replace {
                    tx.replace_subtree(path, entries)?;
                }
                for entry in &s.put {
                    tx.put_entry(entry)?;
                }
                let mut row = s.row.clone();
                for path in &s.child_dirs {
                    if let Some(child) = tx.get(path)? {
                        row.size += child.size;
                        row.files += child.files;
                        row.newest_mtime = row.newest_mtime.max(child.newest_mtime);
                    }
                }
                tx.put_entry(&row)
            }
        }
    }
}

/// Folders only, inside a root, not excluded, one item per folder (a
/// `subtree` change wins over a one-level one). A path that is a file, or
/// that vanished, also brings in its folder, whose listing changed with it.
fn normalize(batch: &EventBatch, roots: &[PathBuf], cfg: &Config) -> Vec<(PathBuf, bool)> {
    // Only the part of the path below its root is checked against the skip
    // list, as the scanner does: a root may itself sit under a system path.
    let inside = |p: &Path| {
        roots.iter().any(|r| {
            p.starts_with(r)
                && !p
                    .ancestors()
                    .take_while(|a| *a != r.as_path())
                    .any(|a| skipped_by(a, &cfg.excludes))
        })
    };
    let mut merged: HashMap<PathBuf, bool> = HashMap::new();
    let mut order = Vec::new();
    let mut push = |p: PathBuf, subtree: bool| {
        if inside(&p) {
            match merged.get_mut(&p) {
                Some(s) => *s |= subtree,
                None => {
                    merged.insert(p.clone(), subtree);
                    order.push(p);
                }
            }
        }
    };
    for change in &batch.changes {
        match fs::symlink_metadata(&change.dir) {
            Ok(meta) if meta.is_dir() => push(change.dir.clone(), change.subtree),
            other => {
                if other.is_err() {
                    push(change.dir.clone(), false);
                }
                if let Some(parent) = change.dir.parent() {
                    push(parent.to_path_buf(), false);
                }
            }
        }
    }
    order
        .into_iter()
        .map(|p| {
            let subtree = merged[&p];
            (p, subtree)
        })
        .collect()
}

fn classify(store: &Store, dir: PathBuf, subtree: bool, roots: &[PathBuf]) -> Result<Work> {
    if fs::symlink_metadata(&dir).is_err() {
        return Ok(Work::Remove(dir));
    }
    if store.get(&dir)?.is_some() {
        return Ok(if subtree {
            Work::Full(dir)
        } else {
            Work::Shallow(dir)
        });
    }
    // No row yet: the new part of the tree starts at the highest folder
    // without one, so scan from there.
    let mut top = dir;
    while let Some(parent) = top.parent() {
        if roots.iter().any(|r| r == &top) || store.get(parent)?.is_some() {
            break;
        }
        top = parent.to_path_buf();
    }
    Ok(Work::Full(top))
}

/// Re-reads `dir`'s direct children: new folders are scanned, gone ones
/// dropped, files re-measured; `dir`'s own row is rebuilt when applied.
fn shallow(store: &Store, dir: &Path, cfg: &Config) -> Result<Plan> {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return Ok(Plan::Remove(dir.to_path_buf()));
    };
    let mut plan = ShallowPlan {
        row: Entry {
            path: dir.to_path_buf(),
            kind: Kind::Dir,
            size: 0,
            files: 0,
            small_files: 0,
            small_bytes: 0,
            newest_mtime: meta.mtime(),
            last_used: store.get(dir)?.and_then(|e| e.last_used),
            flags: Flags::empty(),
        },
        remove: Vec::new(),
        replace: Vec::new(),
        put: Vec::new(),
        child_dirs: Vec::new(),
    };
    let existing: HashMap<PathBuf, Entry> = store
        .children(dir)?
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect();

    let Ok(listing) = fs::read_dir(dir) else {
        plan.remove.extend(existing.into_keys());
        plan.row.flags.insert(Flags::UNREADABLE);
        return Ok(Plan::Shallow(plan));
    };

    let mut on_disk = HashSet::new();
    for child in listing.flatten() {
        let path = child.path();
        if skipped_by(&path, &cfg.excludes) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        on_disk.insert(path.clone());
        let kind = if meta.is_dir() { Kind::Dir } else { Kind::File };
        let old = existing.get(&path);
        let kind_changed = old.is_some_and(|o| o.kind != kind);
        if kind_changed {
            plan.remove.push(path.clone());
        }
        let name = child.file_name();
        if kind == Kind::Dir {
            if name == ".git" {
                plan.row.flags.insert(Flags::GIT);
            }
            if old.is_none() || kind_changed {
                plan.replace.push((path.clone(), scan_subtree(&path, cfg)?));
            }
            plan.child_dirs.push(path);
            continue;
        }
        let size = file_size(&meta);
        plan.row.size += size;
        plan.row.files += 1;
        plan.row.newest_mtime = plan.row.newest_mtime.max(meta.mtime());
        if name == "CACHEDIR.TAG" {
            plan.row.flags.insert(Flags::CACHEDIR);
        }
        if size >= cfg.min_file_size {
            plan.put.push(Entry {
                path,
                kind: Kind::File,
                size,
                files: 1,
                small_files: 0,
                small_bytes: 0,
                newest_mtime: meta.mtime(),
                last_used: old.and_then(|o| o.last_used),
                flags: Flags::empty(),
            });
        } else {
            if old.is_some() && !kind_changed {
                plan.remove.push(path);
            }
            plan.row.small_files += 1;
            plan.row.small_bytes += size;
        }
    }
    plan.remove
        .extend(existing.into_keys().filter(|p| !on_disk.contains(p)));
    Ok(Plan::Shallow(plan))
}
