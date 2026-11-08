//! `settings.json` file adapter: atomic replace, existing mode kept, new file `0600`.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::features::integration::application::SettingsFile;
use crate::features::integration::domain::IntegrationError;
use crate::shared::infrastructure::fs_secure::write_atomic;

pub struct JsonSettingsFile {
    pub path: PathBuf,
}

impl SettingsFile for JsonSettingsFile {
    fn load(&self) -> Result<Option<String>, IntegrationError> {
        match std::fs::read_to_string(&self.path) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(IntegrationError::Io),
        }
    }

    fn save(&self, contents: &str) -> Result<(), IntegrationError> {
        let target = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let mode = std::fs::metadata(&target)
            .map(|m| m.permissions().mode() & 0o777 & !0o022)
            .unwrap_or(0o600);
        if let Some(dir) = target.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|_| IntegrationError::Io)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| IntegrationError::Io)?;
            }
        }
        write_atomic(&target, contents.as_bytes(), mode).map_err(|_| IntegrationError::Io)
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_private_file_and_keeps_existing_mode() {
        let t = tempfile::tempdir().unwrap();
        let f = JsonSettingsFile {
            path: t.path().join(".claude/settings.json"),
        };
        assert_eq!(f.load().unwrap(), None);
        f.save("{}\n").unwrap();
        assert_eq!(
            std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&f.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        f.save("{\"a\":1}\n").unwrap();
        assert_eq!(
            std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(f.load().unwrap().as_deref(), Some("{\"a\":1}\n"));
    }
}
