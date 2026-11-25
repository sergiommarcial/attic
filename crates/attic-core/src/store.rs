//! The SQLite database the service writes and the terminal app reads.
//!
//! One row per folder and one per file at or above the minimum suggestible
//! size. A folder's `size` and `files` cover everything below it, so every
//! write keeps the totals of the folders above it in step.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::{AtticError, Result};

const SCHEMA_VERSION: i64 = 1;
const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE entries (
    path BLOB PRIMARY KEY,
    parent BLOB,
    kind INTEGER NOT NULL,
    size INTEGER NOT NULL,
    files INTEGER NOT NULL,
    small_files INTEGER NOT NULL,
    small_bytes INTEGER NOT NULL,
    newest_mtime INTEGER NOT NULL,
    last_used INTEGER,
    flags INTEGER NOT NULL
);
CREATE INDEX entries_parent ON entries (parent);
CREATE INDEX entries_size ON entries (size);
";

const COLUMNS: &str =
    "path, kind, size, files, small_files, small_bytes, newest_mtime, last_used, flags";

/// Files are only stored at or above the minimum size, so one bound on
/// `size` covers both folders and files and lets SQLite use `entries_size`.
const CANDIDATES_SQL: &str = "SELECT path, kind, size, files, small_files, small_bytes, \
     newest_mtime, last_used, flags FROM entries WHERE size >= ?1";

/// A folder or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A folder.
    Dir,
    /// A file.
    File,
}

/// Facts about a folder or file worth keeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Flags(u32);

impl Flags {
    /// Holds a `CACHEDIR.TAG`: its contents can be rebuilt.
    pub const CACHEDIR: Flags = Flags(1);
    /// Holds a `.git` directory.
    pub const GIT: Flags = Flags(2);
    /// Could not be read; its size is unknown.
    pub const UNREADABLE: Flags = Flags(4);

    /// No flags.
    pub const fn empty() -> Flags {
        Flags(0)
    }

    /// Whether every flag in `other` is set.
    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets the flags in `other`.
    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }
}

/// One stored folder or file. Times are unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Absolute path.
    pub path: PathBuf,
    /// Folder or file.
    pub kind: Kind,
    /// Allocated bytes; for a folder, everything below it.
    pub size: u64,
    /// Files below a folder (1 for a file).
    pub files: u64,
    /// A folder's direct files under the minimum size, which have no row.
    pub small_files: u64,
    /// Their bytes.
    pub small_bytes: u64,
    /// The newest modification time at or below this entry.
    pub newest_mtime: i64,
    /// When it was last used, if Spotlight knows.
    pub last_used: Option<i64>,
    /// See [`Flags`].
    pub flags: Flags,
}

/// The database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

/// Reads and writes inside one transaction; see [`Store::transaction`].
pub struct Tx<'a> {
    conn: &'a Connection,
}

impl Store {
    /// Opens or creates the database at `path`, creating its folder.
    ///
    /// # Errors
    ///
    /// [`AtticError::DbTooNew`] for a database from a newer attic,
    /// [`AtticError::Io`] when the folder cannot be created, otherwise
    /// [`AtticError::Db`].
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AtticError::io(parent, e))?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // Switching to WAL can report "busy" without waiting on the busy
        // handler while another process is opening the same new file.
        let mut attempts = 0;
        while let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
            attempts += 1;
            if attempts == 50 || e.sqlite_error_code() != Some(rusqlite::ErrorCode::DatabaseBusy) {
                return Err(e.into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // A full scan writes a lot at once; don't keep a WAL that size.
        conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
        Store::setup(conn)
    }

    /// An empty database in memory, for tests and one-off runs.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn open_in_memory() -> Result<Store> {
        Store::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<Store> {
        conn.busy_timeout(Duration::from_secs(5))?;
        // Read the version inside a write transaction, so two processes
        // opening a new database at once don't both create the schema.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(AtticError::DbTooNew {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version == 0 {
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        tx.commit()?;
        Ok(Store { conn })
    }

    /// The WAL size SQLite truncates to after a checkpoint, in bytes.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn journal_size_limit(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "journal_size_limit", |r| r.get(0))?)
    }

    /// Runs `f` in one immediate transaction, committed when it returns `Ok`.
    ///
    /// # Errors
    ///
    /// Whatever `f` returns, or [`AtticError::Db`].
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Tx) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&Tx { conn: &tx })?;
        tx.commit()?;
        Ok(value)
    }

    fn reader(&self) -> Tx<'_> {
        Tx { conn: &self.conn }
    }

    /// The entry at `path`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn get(&self, path: &Path) -> Result<Option<Entry>> {
        self.reader().get(path)
    }

    /// A folder's direct children, largest first.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn children(&self, dir: &Path) -> Result<Vec<Entry>> {
        self.reader().children(dir)
    }

    /// The largest folders, largest first.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn largest_dirs(&self, limit: usize) -> Result<Vec<Entry>> {
        query(
            &self.conn,
            &format!("SELECT {COLUMNS} FROM entries WHERE kind = 0 ORDER BY size DESC LIMIT ?1"),
            params![limit as i64],
        )
    }

    /// What suggestions are drawn from: folders and files of at least
    /// `min_size`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn candidates(&self, min_size: u64) -> Result<Vec<Entry>> {
        query(&self.conn, CANDIDATES_SQL, params![min_size as i64])
    }

    /// Rows whose parent has no row: the scan roots as stored.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn tops(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT e.path FROM entries e WHERE e.parent IS NULL
             OR NOT EXISTS (SELECT 1 FROM entries p WHERE p.path = e.parent)",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|r| Ok(PathBuf::from(OsStr::from_bytes(&r?))))
            .collect()
    }

    /// Every entry. Tests compare stores with it; it is not for a real disk.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn all_entries(&self) -> Result<Vec<Entry>> {
        query(
            &self.conn,
            &format!("SELECT {COLUMNS} FROM entries ORDER BY path"),
            [],
        )
    }

    /// Folders that could not be read.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn unreadable(&self) -> Result<Vec<PathBuf>> {
        Ok(query(
            &self.conn,
            &format!("SELECT {COLUMNS} FROM entries WHERE flags & ?1 != 0 ORDER BY path"),
            params![Flags::UNREADABLE.0],
        )?
        .into_iter()
        .map(|e| e.path)
        .collect())
    }

    /// The number of entries.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// A value from the `meta` table.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.reader().meta(key)
    }

    /// Sets a `meta` value.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.transaction(|tx| tx.set_meta(key, value))
    }

    /// See [`Tx::replace_subtree`].
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn replace_subtree(&mut self, root: &Path, entries: &[Entry]) -> Result<()> {
        self.transaction(|tx| tx.replace_subtree(root, entries))
    }

    /// See [`Tx::remove_subtree`].
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn remove_subtree(&mut self, path: &Path) -> Result<u64> {
        self.transaction(|tx| tx.remove_subtree(path))
    }
}

impl Tx<'_> {
    /// The entry at `path`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn get(&self, path: &Path) -> Result<Option<Entry>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM entries WHERE path = ?1"),
                params![bytes(path)],
                entry,
            )
            .optional()?)
    }

    /// A folder's direct children, largest first.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn children(&self, dir: &Path) -> Result<Vec<Entry>> {
        query(
            self.conn,
            &format!("SELECT {COLUMNS} FROM entries WHERE parent = ?1 ORDER BY size DESC, path"),
            params![bytes(dir)],
        )
    }

    /// A value from the `meta` table.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Sets a `meta` value.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Replaces `root` and everything below it with `entries` (which should
    /// include `root` itself), and moves the totals of the folders above
    /// `root` by the difference. Their newest change time rises to `root`'s
    /// if it is later.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn replace_subtree(&self, root: &Path, entries: &[Entry]) -> Result<()> {
        let (old_size, old_files) = self.totals(root)?;
        self.delete_subtree(root)?;
        let mut new = (0, 0, 0);
        for e in entries {
            if e.path == root {
                new = (e.size as i64, e.files as i64, e.newest_mtime);
            }
            self.insert(e)?;
        }
        self.add_to_ancestors(root, new.0 - old_size, new.1 - old_files, new.2)
    }

    /// Writes one row, leaving the rows below it alone, and moves the
    /// totals of the folders above by the difference. Their newest change
    /// time rises to the row's if it is later.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn put_entry(&self, entry: &Entry) -> Result<()> {
        let (old_size, old_files) = self.totals(&entry.path)?;
        self.insert(entry)?;
        self.add_to_ancestors(
            &entry.path,
            entry.size as i64 - old_size,
            entry.files as i64 - old_files,
            entry.newest_mtime,
        )
    }

    fn insert(&self, e: &Entry) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO entries
                 (path, parent, kind, size, files, small_files, small_bytes, newest_mtime, last_used, flags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![
                bytes(&e.path),
                e.path.parent().map(bytes),
                match e.kind {
                    Kind::Dir => 0,
                    Kind::File => 1,
                },
                e.size as i64,
                e.files as i64,
                e.small_files as i64,
                e.small_bytes as i64,
                e.newest_mtime,
                e.last_used,
                e.flags.0,
            ])?;
        Ok(())
    }

    /// Deletes `path` and everything below it, takes its size off the
    /// folders above, and returns the bytes removed (0 if it had no row). The
    /// folders above keep their newest change time: a removal changes its
    /// own folder's time, and a time that is too new only delays a cleanup.
    ///
    /// # Errors
    ///
    /// [`AtticError::Db`].
    pub fn remove_subtree(&self, path: &Path) -> Result<u64> {
        let (size, files) = self.totals(path)?;
        self.delete_subtree(path)?;
        self.add_to_ancestors(path, -size, -files, 0)?;
        Ok(size as u64)
    }

    fn totals(&self, path: &Path) -> Result<(i64, i64)> {
        Ok(self
            .conn
            .query_row(
                "SELECT size, files FROM entries WHERE path = ?1",
                params![bytes(path)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, 0)))
    }

    fn delete_subtree(&self, root: &Path) -> Result<()> {
        let (low, high) = descendant_range(root);
        self.conn.execute(
            "DELETE FROM entries WHERE path = ?1 OR (path >= ?2 AND path < ?3)",
            params![bytes(root), low, high],
        )?;
        Ok(())
    }

    fn add_to_ancestors(&self, path: &Path, size: i64, files: i64, newest: i64) -> Result<()> {
        if size == 0 && files == 0 && newest == 0 {
            return Ok(());
        }
        let mut update = self.conn.prepare_cached(
            "UPDATE entries
             SET size = size + ?1, files = files + ?2, newest_mtime = MAX(newest_mtime, ?3)
             WHERE path = ?4",
        )?;
        for ancestor in path.ancestors().skip(1) {
            update.execute(params![size, files, newest, bytes(ancestor)])?;
        }
        Ok(())
    }
}

fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// Rows strictly below `root` sort between `root/` and `root0` (`0` is the
/// byte after `/`), so a byte range finds them without `LIKE`, whose `%`
/// and `_` would match inside file names.
fn descendant_range(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut low = bytes(root).to_vec();
    if low.last() != Some(&b'/') {
        low.push(b'/');
    }
    let mut high = low.clone();
    if let Some(last) = high.last_mut() {
        *last = b'0';
    }
    (low, high)
}

fn query(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, entry)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn entry(row: &Row) -> rusqlite::Result<Entry> {
    let path: Vec<u8> = row.get(0)?;
    let count = |i| row.get::<_, i64>(i).map(|n| n.max(0) as u64);
    Ok(Entry {
        path: PathBuf::from(OsStr::from_bytes(&path)),
        kind: if row.get::<_, i64>(1)? == 0 {
            Kind::Dir
        } else {
            Kind::File
        },
        size: count(2)?,
        files: count(3)?,
        small_files: count(4)?,
        small_bytes: count(5)?,
        newest_mtime: row.get(6)?,
        last_used: row.get(7)?,
        flags: Flags(row.get(8)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_use_the_size_index() {
        let store = Store::open_in_memory().unwrap();
        let plan: Vec<String> = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", CANDIDATES_SQL))
            .unwrap()
            .query_map([1i64], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(plan.iter().any(|p| p.contains("entries_size")), "{plan:?}");
    }
}
