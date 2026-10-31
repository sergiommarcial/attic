//! How full a volume is.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::{AtticError, Result};

/// A volume's size and free space, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiskUsage {
    /// The volume's size.
    pub total: u64,
    /// Space available to the user.
    pub available: u64,
}

impl DiskUsage {
    /// Space in use, as Finder counts it (size minus available).
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// The usage of the volume holding `path`.
///
/// # Errors
///
/// [`AtticError::Io`] when `statfs` fails.
pub fn usage(path: &Path) -> Result<DiskUsage> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| AtticError::io(path, io::Error::new(io::ErrorKind::InvalidInput, e)))?;
    // SAFETY: `statfs` fills the zeroed struct for a valid C string.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stats) } != 0 {
        return Err(AtticError::io(path, io::Error::last_os_error()));
    }
    let block = u64::from(stats.f_bsize);
    Ok(DiskUsage {
        total: stats.f_blocks * block,
        available: stats.f_bavail * block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_of_temp_dir_is_sane() {
        let dir = tempfile::tempdir().unwrap();
        let u = usage(dir.path()).unwrap();
        assert!(u.total > 0);
        assert!(u.available <= u.total);
        assert_eq!(u.used(), u.total - u.available);
    }

    #[test]
    fn missing_path_is_an_error() {
        assert!(usage(std::path::Path::new("/no/such/attic/path")).is_err());
    }
}
