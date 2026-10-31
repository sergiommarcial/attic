//! The one error type.

use std::io;
use std::path::PathBuf;

/// Everything that can go wrong in attic.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AtticError {
    /// A file system call on `path` failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the call was about.
        path: PathBuf,
        /// What the OS said.
        source: io::Error,
    },
    /// The database failed or holds something this build cannot read.
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    /// The database was written by a newer attic.
    #[error(
        "the database is from a newer attic (schema {found}, this build reads up to {supported})"
    )]
    DbTooNew {
        /// The schema version found.
        found: i64,
        /// The newest version this build reads.
        supported: i64,
    },
    /// The config file is invalid.
    #[error("{path}: {message}")]
    Config {
        /// The config file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
    /// Moving `path` to the Trash failed.
    #[error("could not move {path} to the Trash: {message}")]
    Trash {
        /// The path that stayed put.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// An external command (`launchctl`, `osascript`, ...) failed.
    #[error("{program}: {message}")]
    Command {
        /// The program run.
        program: String,
        /// Its error or stderr.
        message: String,
    },
}

/// `Result` with [`AtticError`].
pub type Result<T> = std::result::Result<T, AtticError>;

impl AtticError {
    /// An [`AtticError::Io`] for `path`.
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        AtticError::Io {
            path: path.into(),
            source,
        }
    }
}
