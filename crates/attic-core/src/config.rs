//! The optional config file, `~/.config/attic/config.toml`.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::units::parse_size;
use crate::{AtticError, Result};

/// What the config file can change. Every key is optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Roots scanned besides the home folder.
    pub roots: Vec<PathBuf>,
    /// Paths never scanned or suggested.
    pub excludes: Vec<PathBuf>,
    /// The smallest file that gets its own row and can be suggested.
    pub min_file_size: u64,
    /// How much could be freed before a notification is worth sending.
    pub notify_threshold: u64,
    /// At most this many notifications a day; 0 turns them off.
    pub notify_max_per_day: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            roots: Vec::new(),
            excludes: Vec::new(),
            min_file_size: 500_000_000,
            notify_threshold: 5_000_000_000,
            notify_max_per_day: 1,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    roots: Option<Vec<String>>,
    excludes: Option<Vec<String>>,
    min_file_size: Option<Size>,
    notify_threshold: Option<Size>,
    notify_max_per_day: Option<u32>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Size {
    Bytes(u64),
    Text(String),
}

impl Config {
    /// Parses config text. `path` is only used in errors; `home` expands a
    /// leading `~`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Config`] for invalid TOML, an unknown key or a size
    /// that does not parse.
    pub fn parse(text: &str, path: &Path, home: &Path) -> Result<Config> {
        let bad = |message: String| AtticError::Config {
            path: path.to_path_buf(),
            message,
        };
        let raw: Raw = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        let size = |key: &str, value: Option<Size>, default: u64| match value {
            None => Ok(default),
            Some(Size::Bytes(n)) => Ok(n),
            Some(Size::Text(t)) => {
                parse_size(&t).ok_or_else(|| bad(format!("{key}: \"{t}\" is not a size")))
            }
        };
        let paths = |list: Option<Vec<String>>| {
            list.unwrap_or_default()
                .iter()
                .map(|p| expand(p, home))
                .collect()
        };
        let defaults = Config::default();
        Ok(Config {
            roots: paths(raw.roots),
            excludes: paths(raw.excludes),
            min_file_size: size("min_file_size", raw.min_file_size, defaults.min_file_size)?,
            notify_threshold: size(
                "notify_threshold",
                raw.notify_threshold,
                defaults.notify_threshold,
            )?,
            notify_max_per_day: raw
                .notify_max_per_day
                .unwrap_or(defaults.notify_max_per_day),
        })
    }

    /// Reads and parses the config file; a missing file is the defaults.
    ///
    /// # Errors
    ///
    /// [`AtticError::Io`] when the file exists but cannot be read, otherwise
    /// as [`Config::parse`].
    pub fn load(path: &Path, home: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text, path, home),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(AtticError::io(path, e)),
        }
    }
}

fn expand(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn parse(text: &str) -> Result<Config> {
        Config::parse(
            text,
            Path::new("/h/.config/attic/config.toml"),
            Path::new("/h"),
        )
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.min_file_size, 500_000_000);
        assert_eq!(c.notify_threshold, 5_000_000_000);
        assert_eq!(c.notify_max_per_day, 1);
        assert!(c.roots.is_empty());
        assert!(c.excludes.is_empty());
    }

    #[test]
    fn parses_every_key() {
        let c = parse(
            r#"
            roots = ["/Volumes/Data", "~/elsewhere"]
            excludes = ["~/keep"]
            min_file_size = "1GB"
            notify_threshold = "10 GB"
            notify_max_per_day = 2
            "#,
        )
        .unwrap();
        assert_eq!(
            c.roots,
            [
                PathBuf::from("/Volumes/Data"),
                PathBuf::from("/h/elsewhere")
            ]
        );
        assert_eq!(c.excludes, [PathBuf::from("/h/keep")]);
        assert_eq!(c.min_file_size, 1_000_000_000);
        assert_eq!(c.notify_threshold, 10_000_000_000);
        assert_eq!(c.notify_max_per_day, 2);
    }

    #[test]
    fn sizes_may_be_plain_bytes() {
        let c = parse("min_file_size = 1234").unwrap();
        assert_eq!(c.min_file_size, 1234);
    }

    #[test]
    fn unset_keys_keep_defaults() {
        let c = parse("notify_max_per_day = 3").unwrap();
        assert_eq!(c.min_file_size, Config::default().min_file_size);
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse("min_fle_size = 5").unwrap_err();
        assert!(matches!(&err, AtticError::Config { .. }), "{err}");
        assert!(err.to_string().contains("min_fle_size"), "{err}");
    }

    #[test]
    fn rejects_bad_sizes() {
        let err = parse(r#"min_file_size = "lots""#).unwrap_err();
        assert!(err.to_string().contains("lots"), "{err}");
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml"), dir.path()).unwrap();
        assert_eq!(c, Config::default());
    }
}
