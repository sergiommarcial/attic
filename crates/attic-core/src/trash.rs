//! Moving things to the Trash: the only code in attic that moves or
//! removes user files.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

use ::trash::TrashContext;
use ::trash::macos::{DeleteMethod, TrashContextExtMacos};

use crate::store::Store;
use crate::{AtticError, Result};

/// Moves a path to the Trash.
pub trait Trasher {
    /// Moves `path` to the Trash.
    ///
    /// # Errors
    ///
    /// [`AtticError::Trash`] when it stays where it is.
    fn trash(&self, path: &Path) -> Result<()>;
}

/// Asks Finder, so "Put Back" works afterwards. The first use may make
/// macOS ask permission to control Finder.
#[derive(Debug, Clone, Copy, Default)]
pub struct FinderTrasher;

impl Trasher for FinderTrasher {
    fn trash(&self, path: &Path) -> Result<()> {
        let mut context = TrashContext::default();
        context.set_delete_method(DeleteMethod::Finder);
        context.delete(path).map_err(|e| AtticError::Trash {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

/// Moves paths into a folder it was given instead of the Trash, and
/// remembers them. For tests.
#[derive(Debug)]
pub struct RecordingTrasher {
    bin: PathBuf,
    moved: RefCell<Vec<PathBuf>>,
}

impl RecordingTrasher {
    /// A trasher that moves things into `bin`, which must exist.
    pub fn new(bin: PathBuf) -> Self {
        RecordingTrasher {
            bin,
            moved: RefCell::new(Vec::new()),
        }
    }

    /// What it has moved, in order.
    pub fn moved(&self) -> Vec<PathBuf> {
        self.moved.borrow().clone()
    }
}

impl Trasher for RecordingTrasher {
    fn trash(&self, path: &Path) -> Result<()> {
        let n = self.moved.borrow().len();
        std::fs::rename(path, self.bin.join(n.to_string())).map_err(|e| AtticError::Trash {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        self.moved.borrow_mut().push(path.to_path_buf());
        Ok(())
    }
}

/// What [`move_to_trash`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashOutcome {
    /// Moved to the Trash; `bytes` come back once the Trash is emptied.
    Moved {
        /// The size it had in the store.
        bytes: u64,
    },
    /// It was no longer there; its rows were dropped.
    AlreadyGone {
        /// The size it had in the store.
        bytes: u64,
    },
}

/// Moves `path` to the Trash and drops it and everything below it from the
/// store. If `path` is already gone, only the rows are dropped. If the
/// trasher fails, nothing changes. Once it is in the Trash the result is
/// `Moved`, even when the store cannot be updated right then.
///
/// # Errors
///
/// [`AtticError::Trash`] from the trasher, [`AtticError::Io`] when the path
/// cannot be checked, [`AtticError::Db`] when the store fails.
pub fn move_to_trash(
    store: &mut Store,
    trasher: &dyn Trasher,
    path: &Path,
) -> Result<TrashOutcome> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let bytes = store.remove_subtree(path)?;
            Ok(TrashOutcome::AlreadyGone { bytes })
        }
        Err(e) => Err(AtticError::io(path, e)),
        Ok(_) => {
            let bytes = store.get(path)?.map_or(0, |e| e.size);
            trasher.trash(path)?;
            // It is in the Trash now, so that is the answer even if the
            // store is busy; the service drops the rows when FSEvents
            // reports the move.
            let _ = store.remove_subtree(path);
            Ok(TrashOutcome::Moved { bytes })
        }
    }
}
