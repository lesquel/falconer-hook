//! `~/.ssh/authorized_keys` adapter. Changes are written atomically (temp
//! file + rename) under an exclusive lock. The file is always left `0600`
//! and its directory `0700` (an existing looser mode is tightened, as
//! sshd's StrictModes expects). A symlinked file is followed so dotfile
//! managers keep working (the real target is rewritten and tightened); a
//! dangling symlink is refused rather than replaced.

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
    /// Creates the directory `0700`, or tightens an existing one to `0700`.
    fn private_dir(dir: &Path) -> Result<(), RegistryError> {
        if !dir.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .create(dir)
                .map_err(|e| io(dir, e))?;
        }
        let mode = fs::metadata(dir)
            .map_err(|e| io(dir, e))?
            .permissions()
            .mode()
            & 0o777;
        if mode != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| io(dir, e))?;
        }
        Ok(())
    }

    /// Resolves the real file and returns its contents and the mode to write (`0600`).
    fn load(&self) -> Result<(PathBuf, String, u32), RegistryError> {
        if let Some(dir) = self.path.parent() {
            Self::private_dir(dir)?;
        }
        let link = fs::symlink_metadata(&self.path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !self.path.exists() {
            if link {
                return Err(RegistryError::Storage(
                    format!("{} is a dangling symlink", self.path.display()),
                    "io_error",
                ));
            }
            return Ok((self.path.clone(), String::new(), 0o600));
        }
        let real = fs::canonicalize(&self.path).map_err(|e| io(&self.path, e))?;
        if !fs::metadata(&real).map_err(|e| io(&real, e))?.is_file() {
            return Err(RegistryError::Storage(
                format!("{} is not a regular file", real.display()),
                "io_error",
            ));
        }
        let text = fs::read_to_string(&real).map_err(|e| io(&real, e))?;
        Ok((real, text, 0o600))
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
    fn preserves_existing_lines_and_tightens_modes() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(".ssh")).unwrap();
        fs::set_permissions(t.path().join(".ssh"), fs::Permissions::from_mode(0o755)).unwrap();
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
            0o600
        );
        assert_eq!(
            fs::metadata(t.path().join(".ssh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(ak.append("a\nb").is_err());
    }

    #[test]
    fn symlinked_file_is_followed_and_its_target_tightened() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(".ssh")).unwrap();
        let real = t.path().join("dotfiles_keys");
        fs::write(&real, "ssh-ed25519 AAAAuser laptop\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        let p = t.path().join(".ssh/authorized_keys");
        std::os::unix::fs::symlink(&real, &p).unwrap();
        let ak = FileAuthorizedKeys {
            path: p.clone(),
            lock_path: t.path().join(".lock"),
        };
        ak.append(&authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01"))
            .unwrap();
        assert!(fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&real)
            .unwrap()
            .contains("openmosh:AAAAAAAAAAAAAAAAAAAA01"));
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn refuses_a_dangling_symlink() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(".ssh")).unwrap();
        let p = t.path().join(".ssh/authorized_keys");
        std::os::unix::fs::symlink(t.path().join("nowhere/keys"), &p).unwrap();
        let ak = FileAuthorizedKeys {
            path: p.clone(),
            lock_path: t.path().join(".lock"),
        };
        assert!(ak
            .append(&authorized_keys_line(KEY, "AAAAAAAAAAAAAAAAAAAA01"))
            .is_err());
        assert!(fs::symlink_metadata(&p).unwrap().file_type().is_symlink());
        assert!(!t.path().join("nowhere").exists());
    }
}
