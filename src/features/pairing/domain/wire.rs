//! Pairing plaintexts. Field order is the wire order.

use serde::{Deserialize, Serialize};

/// QR content (`openmosh://pair?d=<b64u(JSON)>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairOffer {
    pub v: u8,
    pub alias: String,
    pub addr: String,
    pub ip: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_fp: String,
    pub hpk: String,
    pub port: u16,
    pub tok: String,
    pub exp: u64,
}

impl PairOffer {
    pub fn uri(&self) -> String {
        let json = serde_json::to_string(self).expect("offer serializes");
        format!(
            "openmosh://pair?d={}",
            crate::shared::domain::encoding::b64u_encode(json.as_bytes())
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
}
