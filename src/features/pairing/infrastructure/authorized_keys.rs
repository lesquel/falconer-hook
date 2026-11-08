//! `~/.ssh/authorized_keys` adapter. Changes are written atomically (temp
//! file + rename) under an exclusive lock; a new file is created `0600` and
//! a missing `~/.ssh` is created `0700`. Symlinks are followed so dotfile
//! managers keep working.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::features::pairing::domain::{device_comment, AuthorizedKeys, RegistryError};
use crate::shared::infrastructure::fs_secure::{lock_exclusive, write_atomic, FsError};

pub struct FileAuthorizedKeys {
    pub path: PathBuf,
    pub lock_path: PathBuf,
}

fn fs_err(e: FsError) -> RegistryError {
    use crate::shared::domain::error::ErrorCode;
    RegistryError::Storage(e.to_string(), e.code())
}

fn io(path: &Path, e: std::io::Error) -> RegistryError {
    fs_err(FsError::Io(path.display().to_string(), e))
}

impl FileAuthorizedKeys {
    /// Resolves the real file and returns its contents and mode.
    fn load(&self) -> Result<(PathBuf, String, u32), RegistryError> {
        if !self.path.exists() {
            if let Some(dir) = self.path.parent() {
                if !dir.exists() {
                    fs::DirBuilder::new()
                        .recursive(true)
                        .create(dir)
                        .map_err(|e| io(dir, e))?;
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                        .map_err(|e| io(dir, e))?;
                }
            }
            return Ok((self.path.clone(), String::new(), 0o600));
        }
        let real = fs::canonicalize(&self.path).map_err(|e| io(&self.path, e))?;
        let text = fs::read_to_string(&real).map_err(|e| io(&real, e))?;
        let mode = fs::metadata(&real)
            .map_err(|e| io(&real, e))?
            .permissions()
            .mode()
            & 0o777;
        // Never widen; tighten anything group/other-writable.
        Ok((real, text, mode & !0o022))
    }
}

impl AuthorizedKeys for FileAuthorizedKeys {
    fn append(&self, line: &str) -> Result<(), RegistryError> {
        if line.contains(['\n', '\r']) {
            return Err(RegistryError::Storage(
                "invalid key line".into(),
                "invalid_input",
            ));
        }
        let _g = lock_exclusive(&self.lock_path).map_err(fs_err)?;
        let (real, mut text, mode) = self.load()?;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
        write_atomic(&real, text.as_bytes(), mode).map_err(fs_err)
    }

    fn remove_device(&self, device_id: &str) -> Result<bool, RegistryError> {
        let _g = lock_exclusive(&self.lock_path).map_err(fs_err)?;
        if !self.path.exists() {
            return Ok(false);
        }
        let (real, text, mode) = self.load()?;
        let tag = format!(" {}", device_comment(device_id));
        let mut removed = false;
        let mut out = String::with_capacity(text.len());
        for l in text.lines() {
            if l.trim_end().ends_with(&tag) {
                removed = true;
            } else {
                out.push_str(l);
                out.push('\n');
            }
        }
        if removed {
            write_atomic(&real, out.as_bytes(), mode).map_err(fs_err)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::authorized_keys_line;

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH";

    #[test]
    fn creates_ssh_dir_and_file_privately_then_removes_only_its_line() {
        let t = tempfile::tempdir().unwrap();
        let ak = FileAuthorizedKeys {
            path: t.path().join(".ssh/authorized_keys"),
            lock_path: t.path().join(".lock"),
        };
        let line = authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01");
        ak.append(&line).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&t.path().join(".ssh")), 0o700);
        assert_eq!(mode(&ak.path), 0o600);
        assert_eq!(fs::read_to_string(&ak.path).unwrap(), format!("{line}\n"));
        assert!(!ak.remove_device("AAAAAAAAAAAAAAAAAAAA02").unwrap());
        assert!(ak.remove_device("AAAAAAAAAAAAAAAAAAAA01").unwrap());
        assert_eq!(fs::read_to_string(&ak.path).unwrap(), "");
    }

    #[test]
    fn preserves_existing_lines_and_mode() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(".ssh")).unwrap();
        let p = t.path().join(".ssh/authorized_keys");
        fs::write(&p, "ssh-ed25519 AAAAuser laptop").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        let ak = FileAuthorizedKeys {
            path: p.clone(),
            lock_path: t.path().join(".lock"),
        };
        ak.append(&authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01"))
            .unwrap();
        ak.remove_device("AAAAAAAAAAAAAAAAAAAA01").unwrap();
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "ssh-ed25519 AAAAuser laptop\n"
        );
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(ak.append("a\nb").is_err());
    }
}
