//! Filesystem helpers enforcing control T-8: state/config directories are
//! `0700`, files are `0600`, looser modes are refused, and sensitive files are
//! replaced atomically (write temp file in the same directory, fsync, rename).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("{path} has mode {mode:04o}; expected {expected:04o} or stricter (run: chmod {expected:o} {path})")]
    TooOpen {
        path: String,
        mode: u32,
        expected: u32,
    },
    #[error("{0}: {1}")]
    Io(String, io::Error),
}

impl ErrorCode for FsError {
    fn code(&self) -> &'static str {
        match self {
            FsError::TooOpen { .. } => "permissions_too_open",
            FsError::Io(..) => "io_error",
        }
    }
}

fn io_err(path: &Path, e: io::Error) -> FsError {
    FsError::Io(path.display().to_string(), e)
}

/// Checks that `path` grants nothing to group or others.
pub fn check_private(path: &Path, expected: u32) -> Result<(), FsError> {
    let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(FsError::TooOpen {
            path: path.display().to_string(),
            mode,
            expected,
        });
    }
    Ok(())
}

/// Creates `dir` (and parents) with mode `0700` if missing; refuses an
/// existing directory with a looser mode.
pub fn ensure_private_dir(dir: &Path) -> Result<(), FsError> {
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| io_err(dir, e))?;
        // `recursive` applies the mode to the leaf only through the umask; force it.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| io_err(dir, e))?;
    }
    check_private(dir, 0o700)
}

/// Refuses an existing file whose mode is looser than `0600`. Missing is fine.
pub fn check_private_file_if_exists(path: &Path) -> Result<(), FsError> {
    if path.exists() {
        check_private(path, 0o600)
    } else {
        Ok(())
    }
}

/// Creates an empty `0600` file if it does not exist yet.
pub fn touch_private(path: &Path) -> Result<(), FsError> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => check_private(path, 0o600),
        Err(e) => Err(io_err(path, e)),
    }
}

/// Atomically replaces `path` with `contents`, keeping `mode`.
pub fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<(), FsError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let suffix = crate::shared::domain::encoding::b64u_encode(
        &crate::shared::infrastructure::crypto::random_bytes::<6>(),
    );
    let tmp: PathBuf = dir.join(format!(".{name}.{suffix}.tmp"));
    let result = (|| {
        let mut f: File = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        fs::rename(&tmp, path)?;
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok::<(), io::Error>(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(path, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_private_dir_and_refuses_loose_one() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("a/b");
        ensure_private_dir(&d).unwrap();
        assert_eq!(
            fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        let err = ensure_private_dir(&d).unwrap_err();
        assert_eq!(err.code(), "permissions_too_open");
    }

    #[test]
    fn atomic_write_sets_mode_and_replaces() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        write_atomic(&p, b"one", 0o600).unwrap();
        write_atomic(&p, b"two", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers: Vec<_> = fs::read_dir(t.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind");
    }

    #[test]
    fn loose_file_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        fs::write(&p, b"x").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            check_private_file_if_exists(&p).unwrap_err().code(),
            "permissions_too_open"
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        check_private_file_if_exists(&p).unwrap();
        check_private_file_if_exists(&t.path().join("missing")).unwrap();
    }
}
