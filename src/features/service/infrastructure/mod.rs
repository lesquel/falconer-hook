//! Service file adapter.

use std::path::Path;

use crate::features::service::application::ServiceFiles;
use crate::features::service::domain::ServiceError;
use crate::shared::infrastructure::fs_secure::write_atomic;

pub struct FsServiceFiles;

impl ServiceFiles for FsServiceFiles {
    fn write(&self, path: &Path, contents: &str) -> Result<(), ServiceError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| ServiceError::Io)?;
        }
        write_atomic(path, contents.as_bytes(), 0o644).map_err(|_| ServiceError::Io)
    }

    fn remove(&self, path: &Path) -> Result<(), ServiceError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ServiceError::Io),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_removes_idempotently() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("systemd/user/openmosh-hook.service");
        FsServiceFiles.write(&p, "x").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
        FsServiceFiles.remove(&p).unwrap();
        FsServiceFiles.remove(&p).unwrap();
        assert!(!p.exists());
    }
}
