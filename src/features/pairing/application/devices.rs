//! `devices list | revoke | set-endpoint`.

use serde::Serialize;

use std::net::IpAddr;

use crate::features::pairing::domain::{
    authorized_keys_line, AuthorizedKeys, Device, DeviceRegistry, LiveSessions, RegistryError,
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

/// What `devices revoke` did, plus what is left for the user to do.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RevokeReport {
    pub revoked: DeviceView,
    /// Process ids of the SSH sessions that were ended; `null` when they
    /// could not be identified (device paired before addresses were pinned,
    /// or no `/proc`).
    pub sessions_ended: Option<Vec<u32>>,
    /// Lost-phone checklist: done items first, then the user's next steps.
    pub checklist: Vec<String>,
}

/// The lost-phone checklist for a revoked device.
pub fn lost_phone_checklist(name: &str, ips: &[String], ended: Option<&[u32]>) -> Vec<String> {
    let mut c = vec![format!(
        "Done: the SSH key of \"{name}\" was removed from authorized_keys and its push key deleted, so it gets no more notifications."
    )];
    c.push(match ended {
        Some([]) => "Done: no live SSH session from this phone was found.".to_string(),
        Some(p) => format!(
            "Done: ended {} live SSH session(s) from {}.",
            p.len(),
            ips.join(", ")
        ),
        None => "Check: live SSH sessions of this phone could not be identified; run `who` and end any you do not expect (or restart sshd).".to_string(),
    });
    c.push("Next: remove the phone from your tailnet in the Tailscale admin console (Machines > the phone > Remove), so it cannot reach this host at all.".into());
    c.push(
        "Next: if the phone stored SSH keys for other hosts, remove those keys from each host too."
            .into(),
    );
    c.push("Next: run `openmosh-hook doctor` and review ~/.ssh/authorized_keys for keys you do not recognize.".into());
    c
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

    /// Removes the device's `authorized_keys` line and its registry record,
    /// then ends its live SSH sessions (identified by its pinned addresses).
    /// If the registry write fails, the key line is restored.
    pub fn revoke(
        &self,
        id: &str,
        sessions: &dyn LiveSessions,
    ) -> Result<RevokeReport, DeviceAdminError> {
        if !is_valid_id(id) {
            return Err(DeviceAdminError::BadId);
        }
        let device = self.registry.get(id)?;
        self.keys.remove_device(id)?;
        match self.registry.remove(id) {
            Ok(d) => {
                let ips: Vec<IpAddr> = d.peer_ips.iter().filter_map(|i| i.parse().ok()).collect();
                // Without pinned addresses the phone's sessions cannot be told apart.
                let ended = if ips.is_empty() {
                    None
                } else {
                    sessions.end_sessions_from(&ips)
                };
                let checklist = lost_phone_checklist(&d.name, &d.peer_ips, ended.as_deref());
                Ok(RevokeReport {
                    revoked: d.into(),
                    sessions_ended: ended,
                    checklist,
                })
            }
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
    use crate::features::pairing::domain::fakes::{FakeSessions, MemAuthorizedKeys, MemRegistry};
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
        let sessions = FakeSessions::default();
        let r = admin.revoke(ID, &sessions).unwrap();
        assert_eq!(r.revoked.id, ID);
        assert_eq!(r.sessions_ended, None, "no pinned addresses: unknown");
        assert!(sessions.asked.borrow().is_empty());
        assert!(r.checklist[1].starts_with("Check:"));
        assert!(reg.list().unwrap().is_empty());
        assert_eq!(
            keys.lines.borrow().as_slice(),
            ["ssh-ed25519 AAAAuser laptop"]
        );
        assert_eq!(admin.revoke(ID, &sessions).unwrap_err().code(), "not_found");
        assert_eq!(
            admin.revoke("../etc", &sessions).unwrap_err().code(),
            "invalid_input"
        );
    }

    #[test]
    fn revoke_ends_the_phones_live_sessions_and_lists_next_steps() {
        let (reg, keys) = setup();
        let mut d = reg.get(ID).unwrap();
        d.peer_ips = vec!["100.101.1.2".into(), "fd7a:115c:a1e0::1:2".into()];
        reg.update(d).unwrap();
        let sessions = FakeSessions {
            ended: vec![4711],
            ..Default::default()
        };
        let admin = DeviceAdmin {
            registry: &reg,
            keys: &keys,
        };
        let r = admin.revoke(ID, &sessions).unwrap();
        assert_eq!(r.sessions_ended, Some(vec![4711]));
        assert_eq!(
            sessions.asked.borrow().as_slice(),
            [vec![
                "100.101.1.2".parse::<IpAddr>().unwrap(),
                "fd7a:115c:a1e0::1:2".parse().unwrap()
            ]]
        );
        assert_eq!(
            r.checklist[1],
            "Done: ended 1 live SSH session(s) from 100.101.1.2, fd7a:115c:a1e0::1:2."
        );
        assert!(r
            .checklist
            .iter()
            .any(|c| c.contains("Tailscale admin console")));
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
