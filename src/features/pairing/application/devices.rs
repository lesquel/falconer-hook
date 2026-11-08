//! `devices list | revoke | set-endpoint`.

use serde::Serialize;

use crate::features::pairing::domain::{
    authorized_keys_line, AuthorizedKeys, Device, DeviceRegistry, RegistryError,
};
use crate::features::push::domain::validate_endpoint;
use crate::shared::domain::ids::is_valid_id;

/// What `devices list` shows. The push key stays private to the registry.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub paired_at: u64,
    pub endpoint: String,
    pub endpoint_stale: bool,
    pub ssh_authorized: bool,
}

impl From<Device> for DeviceView {
    fn from(d: Device) -> Self {
        Self {
            id: d.id,
            name: d.name,
            paired_at: d.paired_at,
            endpoint: d.endpoint,
            endpoint_stale: d.endpoint_stale,
            ssh_authorized: d.ssh_key.is_some(),
        }
    }
}

pub struct DeviceAdmin<'a, R, A> {
    pub registry: &'a R,
    pub keys: &'a A,
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceAdminError {
    #[error("invalid device id")]
    BadId,
    #[error("invalid endpoint")]
    BadEndpoint,
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

impl crate::shared::domain::error::ErrorCode for DeviceAdminError {
    fn code(&self) -> &'static str {
        match self {
            DeviceAdminError::BadId => "invalid_input",
            DeviceAdminError::BadEndpoint => "invalid_endpoint",
            DeviceAdminError::Registry(e) => e.code(),
        }
    }
}

impl<R: DeviceRegistry, A: AuthorizedKeys> DeviceAdmin<'_, R, A> {
    pub fn list(&self) -> Result<Vec<DeviceView>, RegistryError> {
        Ok(self
            .registry
            .list()?
            .into_iter()
            .map(DeviceView::from)
            .collect())
    }

    /// Removes the device's `authorized_keys` line and its registry record.
    /// If the registry write fails, the key line is restored.
    pub fn revoke(&self, id: &str) -> Result<DeviceView, DeviceAdminError> {
        if !is_valid_id(id) {
            return Err(DeviceAdminError::BadId);
        }
        let device = self.registry.get(id)?;
        self.keys.remove_device(id)?;
        match self.registry.remove(id) {
            Ok(d) => Ok(d.into()),
            Err(e) => {
                if let Some(k) = &device.ssh_key {
                    let _ = self
                        .keys
                        .append(&authorized_keys_line(k, id, &device.peer_ips));
                }
                Err(e.into())
            }
        }
    }

    /// Rotates the push endpoint and clears `endpoint_stale`.
    pub fn set_endpoint(&self, id: &str, endpoint: &str) -> Result<DeviceView, DeviceAdminError> {
        if !is_valid_id(id) {
            return Err(DeviceAdminError::BadId);
        }
        validate_endpoint(endpoint).map_err(|_| DeviceAdminError::BadEndpoint)?;
        let mut d = self.registry.get(id)?;
        d.endpoint = endpoint.to_string();
        d.endpoint_stale = false;
        self.registry.update(d.clone())?;
        Ok(d.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::fakes::{MemAuthorizedKeys, MemRegistry};
    use crate::shared::domain::error::ErrorCode;

    const ID: &str = "AAAAAAAAAAAAAAAAAAAA01";
    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH";

    fn setup() -> (MemRegistry, MemAuthorizedKeys) {
        let reg = MemRegistry::with(vec![Device {
            id: ID.into(),
            name: "P".into(),
            ppk: "k".into(),
            endpoint: "https://a".into(),
            paired_at: 1,
            endpoint_stale: true,
            ssh_key: Some(KEY.into()),
            peer_ips: Vec::new(),
        }]);
        let keys = MemAuthorizedKeys::default();
        keys.lines
            .borrow_mut()
            .push("ssh-ed25519 AAAAuser laptop".into());
        keys.lines
            .borrow_mut()
            .push(authorized_keys_line(KEY, ID, &[]));
        (reg, keys)
    }

    #[test]
    fn revoke_removes_key_line_and_record() {
        let (reg, keys) = setup();
        let admin = DeviceAdmin {
            registry: &reg,
            keys: &keys,
        };
        assert_eq!(admin.revoke(ID).unwrap().id, ID);
        assert!(reg.list().unwrap().is_empty());
        assert_eq!(
            keys.lines.borrow().as_slice(),
            ["ssh-ed25519 AAAAuser laptop"]
        );
        assert_eq!(admin.revoke(ID).unwrap_err().code(), "not_found");
        assert_eq!(admin.revoke("../etc").unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn set_endpoint_validates_and_clears_stale() {
        let (reg, keys) = setup();
        let admin = DeviceAdmin {
            registry: &reg,
            keys: &keys,
        };
        assert_eq!(
            admin
                .set_endpoint(ID, "http://ntfy.sh/x")
                .unwrap_err()
                .code(),
            "invalid_endpoint"
        );
        let v = admin.set_endpoint(ID, "https://ntfy.sh/new").unwrap();
        assert_eq!(
            (v.endpoint.as_str(), v.endpoint_stale),
            ("https://ntfy.sh/new", false)
        );
        assert!(admin.list().unwrap()[0].ssh_authorized);
    }
}
