//! Devices, pairing wire plaintexts, validation rules and ports.

mod rules;
mod wire;

pub use rules::{
    authorized_keys_line, device_comment, normalize_ssh_ed25519, sanitize_name, valid_ppk,
};
pub use wire::{PairOffer, PairRequestPlain, PairResponsePlain};

/// One-time pairing token length: 128 bits keeps the QR small and is far
/// beyond guessing within one 5-minute, 5-failure session.
pub const PAIR_TOKEN_LEN: usize = 16;
pub type PairToken = [u8; PAIR_TOKEN_LEN];

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
    /// Tailnet addresses the key is pinned to (`from=`); empty for devices
    /// paired before v0.2.1 (whole tailnet) and for `pair-ssh`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peer_ips: Vec<String>,
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
    /// Marks `id` stale only while its endpoint is still `endpoint`, in one
    /// step: an endpoint rotated meanwhile (`devices set-endpoint`) is kept
    /// and stays active. Returns whether the flag was set.
    fn mark_stale(&self, id: &str, endpoint: &str) -> Result<bool, RegistryError>;

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
    /// Removes every line tagged `openmosh:` (any device) and returns how
    /// many; with `dry_run` only counts them, changing nothing on disk.
    fn remove_tagged(&self, dry_run: bool) -> Result<usize, RegistryError>;
}

/// Live SSH sessions of this user, found by the peer address they came from.
pub trait LiveSessions {
    /// Ends this user's SSH sessions whose peer is one of `ips`, never the
    /// session the caller itself runs in. Returns the ended session process
    /// ids, or `None` when sessions cannot be inspected on this system.
    fn end_sessions_from(&self, ips: &[IpAddr]) -> Option<Vec<u32>>;
}

/// Whether an `authorized_keys` line belongs to a paired phone.
/// A line OpenMosh wrote: its comment is exactly `openmosh:<device id>`, where the id is
/// 16 random bytes in b64url (22 characters). Hand-written comments that merely start with
/// `openmosh:` are left alone.
pub fn is_tagged_line(line: &str) -> bool {
    line.split_whitespace()
        .last()
        .and_then(|c| c.strip_prefix("openmosh:"))
        .is_some_and(|id| {
            id.len() == 22
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
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

/// The node behind a tailnet address, as `tailscale whois` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// `UserProfile.LoginName` of the owner.
    pub login: String,
    /// MagicDNS name of the node, without the trailing dot.
    pub node: Option<String>,
    /// The node's own tailnet addresses (`Node.Addresses`).
    pub addresses: Vec<IpAddr>,
}

/// Read-only Tailscale queries.
pub trait Tailnet {
    fn status(&self) -> Result<TailnetStatus, TailnetError>;
    fn ipv4(&self) -> Result<Ipv4Addr, TailnetError>;
    /// The user-owned node behind `ip`, if any (tagged nodes have none).
    fn whois(&self, ip: IpAddr) -> Result<Option<Peer>, TailnetError>;
}

/// The host user's go-ahead before a phone's key is authorized.
pub trait PairConfirm {
    /// `true` only when the user said yes to `Pair "<name>" from <node> (<ip>)?`.
    fn confirm(&self, name: &str, node: &str, ip: IpAddr) -> bool;
}

/// Request decryption / response encryption for the QR flow (wire contract §6.3).
pub trait PairCrypto {
    type Session;
    /// Parses and decrypts one request line with keys derived from `token`.
    fn open_request(
        &self,
        line: &str,
        token: &PairToken,
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
        fn mark_stale(&self, id: &str, endpoint: &str) -> Result<bool, RegistryError> {
            let mut v = self.0.borrow_mut();
            match v.iter_mut().find(|d| d.id == id && d.endpoint == endpoint) {
                Some(d) => {
                    d.endpoint_stale = true;
                    Ok(true)
                }
                None => Ok(false),
            }
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

    /// Records which addresses it was asked to cut off.
    #[derive(Default)]
    pub struct FakeSessions {
        pub asked: RefCell<Vec<Vec<IpAddr>>>,
        pub ended: Vec<u32>,
        pub unsupported: bool,
    }

    impl LiveSessions for FakeSessions {
        fn end_sessions_from(&self, ips: &[IpAddr]) -> Option<Vec<u32>> {
            self.asked.borrow_mut().push(ips.to_vec());
            (!self.unsupported).then(|| self.ended.clone())
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
        fn remove_tagged(&self, dry_run: bool) -> Result<usize, RegistryError> {
            let mut v = self.lines.borrow_mut();
            let n = v.iter().filter(|l| is_tagged_line(l)).count();
            if !dry_run {
                v.retain(|l| !is_tagged_line(l));
            }
            Ok(n)
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
        pub whois_calls: std::cell::Cell<u32>,
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
                whois_calls: Default::default(),
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
        fn whois(&self, ip: IpAddr) -> Result<Option<Peer>, TailnetError> {
            self.whois_calls.set(self.whois_calls.get() + 1);
            if self.whois_fails {
                return Err(TailnetError::Failed);
            }
            Ok(self.peers.get(&ip).map(|login| Peer {
                login: login.clone(),
                node: Some("pixel-8.tail.ts.net".into()),
                addresses: vec![ip, "fd7a:115c:a1e0::1:2".parse().unwrap()],
            }))
        }
    }

    #[test]
    fn only_lines_with_a_device_id_comment_are_ours() {
        assert!(is_tagged_line(
            "ssh-ed25519 AAAA openmosh:AAAAAAAAAAAAAAAAAAAA01"
        ));
        assert!(is_tagged_line(
            "from=\"100.64.0.0/10\" ssh-ed25519 AAAA openmosh:abc-DEF_1234567890abcd"
        ));
        assert!(!is_tagged_line("ssh-ed25519 BBBB openmosh:mine-by-hand"));
        assert!(!is_tagged_line("ssh-ed25519 BBBB openmosh:"));
        assert!(!is_tagged_line("ssh-ed25519 BBBB laptop"));
        assert!(!is_tagged_line(
            "ssh-ed25519 BBBB openmosh:AAAAAAAAAAAAAAAAAAAA0!"
        ));
    }
}
