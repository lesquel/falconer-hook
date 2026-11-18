//! A private place to download an official installer before running it.

use rand_core::{OsRng, RngCore};
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// A fresh `0700` directory under the temp dir holding one empty `0600`
/// file. Removed with everything in it when dropped.
pub struct PrivateTempFile {
    dir: PathBuf,
    file: PathBuf,
}

impl PrivateTempFile {
    /// Created atomically with a random name, so another local user can
    /// neither predict it nor pre-create it.
    pub fn new(name: &str) -> std::io::Result<Self> {
        let base = std::env::temp_dir();
        let mut last = None;
        for _ in 0..8 {
            let mut b = [0u8; 8];
            OsRng.fill_bytes(&mut b);
            let suffix: String = b.iter().map(|x| format!("{x:02x}")).collect();
            let dir = base.join(format!("falconer-installer-{suffix}"));
            match DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    let file = dir.join(name);
                    let made = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&file);
                    return match made {
                        Ok(_) => Ok(Self { dir, file }),
                        Err(e) => {
                            let _ = std::fs::remove_dir_all(&dir);
                            Err(e)
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("no temp directory")))
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Whether the download left something to run.
    pub fn has_content(&self) -> bool {
        std::fs::metadata(&self.file).is_ok_and(|m| m.is_file() && m.len() > 0)
    }
}

impl Drop for PrivateTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_and_removed_on_drop() {
        let t = PrivateTempFile::new("install.sh").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = t.path().parent().unwrap().to_path_buf();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(t.path()), 0o600);
        assert!(!t.has_content());
        std::fs::write(t.path(), "echo hi\n").unwrap();
        assert!(t.has_content());
        assert_eq!(mode(t.path()), 0o600, "writing keeps the mode");
        let other = PrivateTempFile::new("install.sh").unwrap();
        assert_ne!(other.path(), t.path());
        drop(t);
        assert!(!dir.exists());
    }
}
