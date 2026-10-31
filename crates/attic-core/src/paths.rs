//! Where attic keeps its files, all derived from the home folder.

use std::path::{Path, PathBuf};

use crate::{AtticError, Result};

/// The files attic reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The user's home folder.
    pub home: PathBuf,
    /// The SQLite database.
    pub db: PathBuf,
    /// The optional config file.
    pub config: PathBuf,
    /// Where launchd looks for per-user agents.
    pub launch_agents: PathBuf,
    /// Where the service's stdout and stderr go.
    pub logs: PathBuf,
}

impl Paths {
    /// The paths for a home folder.
    pub fn from_home(home: &Path) -> Paths {
        Paths {
            home: home.to_path_buf(),
            db: home.join("Library/Application Support/attic/attic.db"),
            config: home.join(".config/attic/config.toml"),
            launch_agents: home.join("Library/LaunchAgents"),
            logs: home.join("Library/Logs/attic"),
        }
    }

    /// The paths for `$HOME`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Config`] when `HOME` is unset or empty.
    pub fn from_env() -> Result<Paths> {
        match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => Ok(Paths::from_home(Path::new(&home))),
            _ => Err(AtticError::Config {
                path: PathBuf::from("$HOME"),
                message: "HOME is not set".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn everything_lives_under_home() {
        let p = Paths::from_home(Path::new("/h"));
        assert_eq!(p.home, Path::new("/h"));
        assert_eq!(
            p.db,
            Path::new("/h/Library/Application Support/attic/attic.db")
        );
        assert_eq!(p.config, Path::new("/h/.config/attic/config.toml"));
        assert_eq!(p.launch_agents, Path::new("/h/Library/LaunchAgents"));
        assert_eq!(p.logs, Path::new("/h/Library/Logs/attic"));
    }
}
