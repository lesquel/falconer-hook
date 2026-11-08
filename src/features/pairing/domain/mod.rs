//! Devices, pairing wire plaintexts, validation rules and ports.

mod rules;
mod wire;

pub use rules::{
    authorized_keys_line, device_comment, normalize_ssh_ed25519, sanitize_name, valid_ppk,
};
pub use wire::{PairOffer, PairRequestPlain, PairResponsePlain};

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// b64u X25519 push public key.
    pub ppk: String,
    pub endpoint: String,
    pub paired_at: u64,
    #[serde(default)]
    pub endpoint_stale: bool,
    /// Normalized `ssh-ed25519 <b64>` authorized at pairing (QR flow only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<String>,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("device not found")]
    NotFound,
    #[error("device registry is corrupt")]
    Corrupt,
    #[error("{0}")]
    Storage(String, &'static str),
}

impl ErrorCode for RegistryError {
    fn code(&self) -> &'static str {
        match self {
            RegistryError::NotFound => "not_found",
            RegistryError::Corrupt => "registry_corrupt",
            RegistryError::Storage(_, code) => code,
        }
    }
}

/// Persistent list of paired devices. Every write is atomic.
pub trait DeviceRegistry {
    fn list(&self) -> Result<Vec<Device>, RegistryError>;
    fn add(&self, device: Device) -> Result<(), RegistryError>;
    /// Removes and returns the device.
    fn remove(&self, id: &str) -> Result<Device, RegistryError>;
    fn update(&self, device: Device) -> Result<(), RegistryError>;

    fn get(&self, id: &str) -> Result<Device, RegistryError> {
        self.list()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or(RegistryError::NotFound)
    }
}

/// `~/.ssh/authorized_keys`, written atomically.
pub trait AuthorizedKeys {
    fn append(&self, line: &str) -> Result<(), RegistryError>;
    /// Removes every line ending in `openmosh:<device_id>`. Returns whether one was removed.
    fn remove_device(&self, device_id: &str) -> Result<bool, RegistryError>;
}

/// Snapshot of the local Tailscale node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TailnetStatus {
    pub running: bool,
    pub login_name: Option<String>,
    /// MagicDNS name without the trailing dot.
    pub dns_name: Option<String>,
    /// The node is owned by ACL tags rather than a user.
    pub tagged: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TailnetError {
    #[error("tailscale is not installed")]
    Missing,
    #[error("tailscale query failed")]
    Failed,
}

/// Read-only Tailscale queries.
pub trait Tailnet {
    fn status(&self) -> Result<TailnetStatus, TailnetError>;
    fn ipv4(&self) -> Result<Ipv4Addr, TailnetError>;
    /// `UserProfile.LoginName` of the node owning `ip`, if any.
    fn whois_login(&self, ip: IpAddr) -> Result<Option<String>, TailnetError>;
}

/// Request decryption / response encryption for the QR flow (wire contract §6.3).
pub trait PairCrypto {
    type Session;
    /// Parses and decrypts one request line with keys derived from `token`.
    fn open_request(
        &self,
        line: &str,
        token: &[u8; 32],
    ) -> Option<(PairRequestPlain, Self::Session)>;
    fn seal_response(&self, session: &Self::Session, response: &PairResponsePlain) -> String;
}

#[cfg(test)]
pub mod fakes {
    use super::*;
    use std::cell::RefCell;

    pub use super::tailnet_fakes::FakeTailnet;

    /// In-memory registry for use-case tests.
    #[derive(Default)]
    pub struct MemRegistry(pub RefCell<Vec<Device>>);

    impl MemRegistry {
        pub fn with(devices: Vec<Device>) -> Self {
            Self(RefCell::new(devices))
        }
    }

    impl DeviceRegistry for MemRegistry {
        fn list(&self) -> Result<Vec<Device>, RegistryError> {
            Ok(self.0.borrow().clone())
        }
        fn add(&self, device: Device) -> Result<(), RegistryError> {
            self.0.borrow_mut().push(device);
            Ok(())
        }
        fn remove(&self, id: &str) -> Result<Device, RegistryError> {
            let mut v = self.0.borrow_mut();
            let i = v
                .iter()
                .position(|d| d.id == id)
                .ok_or(RegistryError::NotFound)?;
            Ok(v.remove(i))
        }
        fn update(&self, device: Device) -> Result<(), RegistryError> {
            let mut v = self.0.borrow_mut();
            let slot = v
                .iter_mut()
                .find(|d| d.id == device.id)
                .ok_or(RegistryError::NotFound)?;
            *slot = device;
            Ok(())
        }
    }

    /// In-memory authorized_keys.
    #[derive(Default)]
    pub struct MemAuthorizedKeys {
        pub lines: RefCell<Vec<String>>,
        pub fail_append: bool,
    }

    impl AuthorizedKeys for MemAuthorizedKeys {
        fn append(&self, line: &str) -> Result<(), RegistryError> {
            if self.fail_append {
                return Err(RegistryError::Storage("append failed".into(), "io_error"));
            }
            self.lines.borrow_mut().push(line.into());
            Ok(())
        }
        fn remove_device(&self, device_id: &str) -> Result<bool, RegistryError> {
            let tag = format!(" {}", device_comment(device_id));
            let mut v = self.lines.borrow_mut();
            let before = v.len();
            v.retain(|l| !l.ends_with(&tag));
            Ok(v.len() != before)
        }
    }
}

#[cfg(test)]
pub mod tailnet_fakes {
    use super::*;
    use std::collections::HashMap;

    /// Scripted Tailscale: node `100.88.77.66`, MagicDNS `host.tail.ts.net`.
    pub struct FakeTailnet {
        pub status: TailnetStatus,
        pub peers: HashMap<IpAddr, String>,
        pub missing: bool,
        pub whois_fails: bool,
    }

    impl FakeTailnet {
        pub fn running(login: &str, peers: &[(&str, &str)]) -> Self {
            Self {
                status: TailnetStatus {
                    running: true,
                    login_name: Some(login.into()),
                    dns_name: Some("host.tail.ts.net".into()),
                    tagged: false,
                },
                peers: peers
                    .iter()
                    .map(|(ip, l)| (ip.parse().unwrap(), l.to_string()))
                    .collect(),
                missing: false,
                whois_fails: false,
            }
        }
    }

    impl Tailnet for FakeTailnet {
        fn status(&self) -> Result<TailnetStatus, TailnetError> {
            if self.missing {
                return Err(TailnetError::Missing);
            }
            Ok(self.status.clone())
        }
        fn ipv4(&self) -> Result<Ipv4Addr, TailnetError> {
            Ok("100.88.77.66".parse().unwrap())
        }
        fn whois_login(&self, ip: IpAddr) -> Result<Option<String>, TailnetError> {
            if self.whois_fails {
                return Err(TailnetError::Failed);
            }
            Ok(self.peers.get(&ip).cloned())
        }
    }
}
