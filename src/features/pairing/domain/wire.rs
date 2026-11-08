//! Pairing plaintexts. Field order is the wire order.

use serde::{Deserialize, Serialize};

/// QR content, wire version 3 (TRD §6.3 step 4). Only what the phone needs
/// to reach the host and authenticate it, so the code stays small:
///
/// `openmosh://pair?v=3&i=<ip>&k=<hpk>&p=<port>&t=<tok>`
///
/// `k` (host X25519 public key) and `t` (16-byte token) are b64url. Alias, SSH
/// user, port and host key fingerprint travel in the encrypted response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairOffer {
    pub v: u8,
    pub ip: String,
    pub hpk: String,
    pub port: u16,
    pub tok: String,
}

impl PairOffer {
    pub fn uri(&self) -> String {
        format!(
            "openmosh://pair?v={}&i={}&k={}&p={}&t={}",
            self.v, self.ip, self.hpk, self.port, self.tok
        )
    }
}

/// Decrypted request from the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequestPlain {
    pub tok: String,
    pub name: String,
    pub ssh_pub: String,
    pub ppk: String,
    pub endpoint: String,
}

/// Plaintext of the encrypted success response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairResponsePlain {
    pub device_id: String,
    pub alias: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub ssh_fp: String,
}
