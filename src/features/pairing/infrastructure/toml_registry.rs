//! `DeviceRegistry` in `devices.toml` (`0600`). Read-modify-write cycles hold
//! an exclusive lock and replace the file atomically (control T-8).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::features::pairing::domain::{Device, DeviceRegistry, RegistryError};
use crate::shared::infrastructure::fs_secure::{
    check_private, lock_exclusive, write_atomic, FsError,
};

pub struct TomlDeviceRegistry {
    pub path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default, rename = "device")]
    devices: Vec<Device>,
}

fn fs_err(e: FsError) -> RegistryError {
    use crate::shared::domain::error::ErrorCode;
    RegistryError::Storage(e.to_string(), e.code())
}

impl TomlDeviceRegistry {
    fn read(&self) -> Result<Vec<Device>, RegistryError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        check_private(&self.path, 0o600).map_err(fs_err)?;
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| fs_err(FsError::Io(self.path.display().to_string(), e)))?;
        let f: File = toml::from_str(&text).map_err(|_| RegistryError::Corrupt)?;
        Ok(f.devices)
    }

    fn write(&self, devices: Vec<Device>) -> Result<(), RegistryError> {
        let text = toml::to_string(&File { devices }).map_err(|_| RegistryError::Corrupt)?;
        write_atomic(&self.path, text.as_bytes(), 0o600).map_err(fs_err)
    }

    fn modify<T>(
        &self,
        f: impl FnOnce(&mut Vec<Device>) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let _guard = lock_exclusive(&self.lock_path).map_err(fs_err)?;
        let mut devices = self.read()?;
        let out = f(&mut devices)?;
        self.write(devices)?;
        Ok(out)
    }
}

impl DeviceRegistry for TomlDeviceRegistry {
    fn list(&self) -> Result<Vec<Device>, RegistryError> {
        self.read()
    }

    fn add(&self, device: Device) -> Result<(), RegistryError> {
        self.modify(|v| {
            v.retain(|d| d.id != device.id);
            v.push(device);
            Ok(())
        })
    }

    fn remove(&self, id: &str) -> Result<Device, RegistryError> {
        self.modify(|v| {
            let i = v
                .iter()
                .position(|d| d.id == id)
                .ok_or(RegistryError::NotFound)?;
            Ok(v.remove(i))
        })
    }

    fn update(&self, device: Device) -> Result<(), RegistryError> {
        self.modify(|v| {
            let slot = v
                .iter_mut()
                .find(|d| d.id == device.id)
                .ok_or(RegistryError::NotFound)?;
            *slot = device;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::error::ErrorCode;
    use std::os::unix::fs::PermissionsExt;

    fn dev(id: &str) -> Device {
        Device {
            id: id.into(),
            name: "Pixel \"8\"".into(),
            ppk: "k".into(),
            endpoint: "https://ntfy.sh/up?x=1".into(),
            paired_at: 3,
            endpoint_stale: false,
            ssh_key: Some("ssh-ed25519 AAAA".into()),
        }
    }

    fn reg(t: &tempfile::TempDir) -> TomlDeviceRegistry {
        TomlDeviceRegistry {
            path: t.path().join("devices.toml"),
            lock_path: t.path().join(".lock"),
        }
    }

    #[test]
    fn round_trips_with_private_mode() {
        let t = tempfile::tempdir().unwrap();
        let r = reg(&t);
        assert!(r.list().unwrap().is_empty());
        r.add(dev("a")).unwrap();
        r.add(dev("b")).unwrap();
        let mut b = dev("b");
        b.endpoint_stale = true;
        r.update(b.clone()).unwrap();
        assert_eq!(r.list().unwrap(), vec![dev("a"), b]);
        assert_eq!(
            std::fs::metadata(&r.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(r.remove("a").unwrap().id, "a");
        assert!(matches!(r.remove("a"), Err(RegistryError::NotFound)));
    }

    #[test]
    fn loose_or_corrupt_file_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let r = reg(&t);
        r.add(dev("a")).unwrap();
        std::fs::set_permissions(&r.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(r.list().unwrap_err().code(), "permissions_too_open");
        std::fs::set_permissions(&r.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&r.path, "not = [valid").unwrap();
        assert_eq!(r.list().unwrap_err().code(), "registry_corrupt");
    }
}
