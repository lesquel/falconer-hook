//! QR pairing over a one-shot tailnet listener (wire contract §6.3).
//!
//! Validation order for every connection, each failure answered with the
//! same generic `pair_rejected`:
//! 1. peer IP in `100.64.0.0/10`
//! 2. `tailscale whois` of the peer has the host's `LoginName`
//! 3. the request decrypts (keys derived from the token)
//! 4. `tok` matches (constant time)
//! 5. not expired
//! 6. not already used
//! 7. `ssh_pub` is a valid `ssh-ed25519` key
//! 8. `endpoint` passes push endpoint validation
//!
//! After 5 failed attempts from a verified peer (checks 3 onwards) the
//! session closes; refusals at checks 1-2 do not count.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::features::pairing::domain::{
    authorized_keys_line, normalize_ssh_ed25519, sanitize_name, valid_ppk, AuthorizedKeys, Device,
    DeviceRegistry, PairCrypto, PairOffer, PairResponsePlain, Tailnet, TailnetError,
};
use crate::features::push::domain::validate_endpoint;
use crate::shared::domain::clock::Clock;
use crate::shared::domain::encoding::{b64u_encode, ct_eq};
use crate::shared::domain::error::ErrorCode;
use crate::shared::domain::ids::IdGenerator;
use crate::shared::domain::net::is_tailnet_ip;

pub const TOKEN_TTL_SECS: u64 = 300;
pub const MAX_FAILURES: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairError {
    #[error("tailscale is not installed")]
    TailscaleMissing,
    #[error("tailscale is not running; run `tailscale up` first")]
    TailscaleNotRunning,
    #[error("could not determine this host's Tailscale identity")]
    NoTailnetIdentity,
    #[error("this host is a tagged Tailscale node; pairing needs a node owned by your user")]
    TaggedHost,
    #[error("SSH host key not readable; is sshd installed?")]
    NoSshHostKey,
    #[error("invalid pairing request")]
    InvalidInput,
    #[error("pairing failed")]
    Storage,
}

impl ErrorCode for PairError {
    fn code(&self) -> &'static str {
        match self {
            PairError::TailscaleMissing => "tailscale_missing",
            PairError::TailscaleNotRunning => "tailscale_not_running",
            PairError::NoTailnetIdentity => "tailscale_error",
            PairError::TaggedHost => "tailscale_tagged",
            PairError::NoSshHostKey => "ssh_host_key_missing",
            PairError::InvalidInput => "invalid_input",
            PairError::Storage => "storage_error",
        }
    }
}

impl From<TailnetError> for PairError {
    fn from(e: TailnetError) -> Self {
        match e {
            TailnetError::Missing => PairError::TailscaleMissing,
            TailnetError::Failed => PairError::NoTailnetIdentity,
        }
    }
}

/// Tailscale facts gathered before opening the listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    pub ipv4: Ipv4Addr,
    pub login_name: String,
    pub dns_name: Option<String>,
}

impl Preflight {
    /// Refuses unless `BackendState == "Running"`, the node is not tagged and
    /// it has a user identity.
    pub fn check(tailnet: &impl Tailnet) -> Result<Self, PairError> {
        let status = tailnet.status()?;
        if !status.running {
            return Err(PairError::TailscaleNotRunning);
        }
        if status.tagged {
            return Err(PairError::TaggedHost);
        }
        let login_name = status.login_name.ok_or(PairError::NoTailnetIdentity)?;
        let ipv4 = tailnet.ipv4()?;
        Ok(Self {
            ipv4,
            login_name,
            dns_name: status.dns_name,
        })
    }
}

/// Host values placed in the QR.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub alias: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub ssh_fp: String,
    pub hpk: [u8; 32],
}

/// Result of one connection.
#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    /// Device stored and key authorized; `reply` is the encrypted response line.
    Paired { device: Device, reply: String },
    /// `reason` is for the local log only; the peer always gets the generic reply.
    Rejected { reason: &'static str },
}

pub struct QrPairing<'a, T, C: PairCrypto, R, A, K, I> {
    pub tailnet: &'a T,
    pub crypto: &'a C,
    pub registry: &'a R,
    pub keys: &'a A,
    pub clock: &'a K,
    pub ids: &'a I,
    preflight: Preflight,
    facts: HostFacts,
    token: Zeroizing<[u8; 32]>,
    expires_at: u64,
    used: bool,
    failures: u32,
}

impl<'a, T, C, R, A, K, I> QrPairing<'a, T, C, R, A, K, I>
where
    T: Tailnet,
    C: PairCrypto,
    R: DeviceRegistry,
    A: AuthorizedKeys,
    K: Clock,
    I: IdGenerator,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tailnet: &'a T,
        crypto: &'a C,
        registry: &'a R,
        keys: &'a A,
        clock: &'a K,
        ids: &'a I,
        preflight: Preflight,
        facts: HostFacts,
        token: Zeroizing<[u8; 32]>,
        ttl_secs: u64,
    ) -> Self {
        let expires_at = clock.now() + ttl_secs.min(TOKEN_TTL_SECS);
        Self {
            tailnet,
            crypto,
            registry,
            keys,
            clock,
            ids,
            preflight,
            facts,
            token,
            expires_at,
            used: false,
            failures: 0,
        }
    }

    /// QR content for a listener bound on `port`.
    pub fn offer(&self, port: u16) -> PairOffer {
        let ip = self.preflight.ipv4.to_string();
        PairOffer {
            v: 1,
            alias: self.facts.alias.clone(),
            addr: self
                .preflight
                .dns_name
                .clone()
                .unwrap_or_else(|| ip.clone()),
            ip,
            ssh_port: self.facts.ssh_port,
            ssh_user: self.facts.ssh_user.clone(),
            ssh_fp: self.facts.ssh_fp.clone(),
            hpk: b64u_encode(&self.facts.hpk),
            port,
            tok: b64u_encode(self.token.as_slice()),
            exp: self.expires_at,
        }
    }

    /// The listener must stop: paired, expired, or too many failures.
    pub fn is_closed(&self) -> bool {
        self.used || self.failures >= MAX_FAILURES || self.clock.now() > self.expires_at
    }

    /// Time left before the token expires.
    pub fn remaining(&self) -> Duration {
        Duration::from_secs(self.expires_at.saturating_sub(self.clock.now()))
    }

    /// Burns the token (timeout, Ctrl+C or success).
    pub fn burn(&mut self) {
        self.used = true;
        *self.token = [0u8; 32];
    }

    fn reject(&mut self, reason: &'static str) -> Attempt {
        self.failures += 1;
        Attempt::Rejected { reason }
    }

    /// Handles one connection. `read_line` is only called once the peer passed
    /// the network checks, so nothing is read from an unverified peer.
    pub fn handle(&mut self, peer: IpAddr, read_line: impl FnOnce() -> Option<String>) -> Attempt {
        if self.failures >= MAX_FAILURES {
            return Attempt::Rejected {
                reason: "too_many_failures",
            };
        }
        // Network-level refusals do not spend the failure budget, so other
        // nodes cannot lock the owner's phone out; they never get to send a
        // byte that is read, and the session still ends at expiry.
        if !is_tailnet_ip(peer) {
            return Attempt::Rejected {
                reason: "not_tailnet",
            };
        }
        match self.tailnet.whois_login(peer) {
            Ok(Some(login)) if login == self.preflight.login_name => {}
            Ok(_) => {
                return Attempt::Rejected {
                    reason: "whois_mismatch",
                }
            }
            Err(_) => {
                return Attempt::Rejected {
                    reason: "whois_failed",
                }
            }
        }
        let Some(line) = read_line() else {
            return self.reject("read_failed");
        };
        let Some((req, session)) = self.crypto.open_request(&line, &self.token) else {
            return self.reject("decrypt_failed");
        };
        let tok_ok = crate::shared::domain::encoding::b64u_decode(&req.tok)
            .is_some_and(|t| ct_eq(&t, self.token.as_slice()));
        if !tok_ok {
            return self.reject("token_mismatch");
        }
        let now = self.clock.now();
        if now > self.expires_at {
            return self.reject("expired");
        }
        if self.used {
            return self.reject("already_used");
        }
        let Some(ssh_key) = normalize_ssh_ed25519(&req.ssh_pub) else {
            return self.reject("bad_ssh_key");
        };
        if validate_endpoint(&req.endpoint).is_err() {
            return self.reject("bad_endpoint");
        }
        let Some(name) = sanitize_name(&req.name) else {
            return self.reject("bad_name");
        };
        if !valid_ppk(&req.ppk) {
            return self.reject("bad_ppk");
        }

        let device = Device {
            id: self.ids.new_id(),
            name,
            ppk: req.ppk,
            endpoint: req.endpoint,
            paired_at: now,
            endpoint_stale: false,
            ssh_key: Some(ssh_key.clone()),
        };
        if self
            .keys
            .append(&authorized_keys_line(&ssh_key, &device.id))
            .is_err()
        {
            return self.reject("storage");
        }
        if self.registry.add(device.clone()).is_err() {
            let _ = self.keys.remove_device(&device.id);
            return self.reject("storage");
        }
        let reply = self.crypto.seal_response(
            &session,
            &PairResponsePlain {
                device_id: device.id.clone(),
                alias: self.facts.alias.clone(),
            },
        );
        // Single use: any later attempt fails the `already_used` check.
        self.used = true;
        Attempt::Paired { device, reply }
    }
}

/// What the TCP listener drives; implemented by `QrPairing`.
pub trait PairSession {
    fn handle_connection(
        &mut self,
        peer: IpAddr,
        read_line: &mut dyn FnMut() -> Option<String>,
    ) -> Attempt;
    fn is_closed(&self) -> bool;
    /// Time left before the session expires; bounds each connection.
    fn remaining(&self) -> Duration;
    fn burn(&mut self);
}

impl<T, C, R, A, K, I> PairSession for QrPairing<'_, T, C, R, A, K, I>
where
    T: Tailnet,
    C: PairCrypto,
    R: DeviceRegistry,
    A: AuthorizedKeys,
    K: Clock,
    I: IdGenerator,
{
    fn handle_connection(
        &mut self,
        peer: IpAddr,
        read_line: &mut dyn FnMut() -> Option<String>,
    ) -> Attempt {
        self.handle(peer, read_line)
    }
    fn is_closed(&self) -> bool {
        QrPairing::is_closed(self)
    }
    fn remaining(&self) -> Duration {
        QrPairing::remaining(self)
    }
    fn burn(&mut self) {
        QrPairing::burn(self)
    }
}

#[cfg(test)]
mod tests;
