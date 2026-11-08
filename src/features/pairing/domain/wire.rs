//! Pairing plaintexts. Field order is the wire order.

use serde::{Deserialize, Serialize};

/// QR content, wire version 2 (TRD §6.3 step 4). A flat query string keeps
/// the code small enough to scan from a terminal:
///
/// `openmosh://pair?v=2&n=<alias>&i=<ip>&s=<ssh_port>&u=<ssh_user>&f=<fp>&k=<hpk>&p=<port>&t=<tok>&e=<exp>`
///
/// `f` is the SHA256 host key fingerprint without the `SHA256:` prefix, in
/// the b64url alphabet; `k` and `t` are b64url. Text values are form-encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairOffer {
    pub v: u8,
    pub alias: String,
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
        let fp: String = self
            .ssh_fp
            .strip_prefix("SHA256:")
            .unwrap_or(&self.ssh_fp)
            .chars()
            .map(|c| match c {
                '+' => '-',
                '/' => '_',
                c => c,
            })
            .collect();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("v", &self.v.to_string())
            .append_pair("n", &self.alias)
            .append_pair("i", &self.ip)
            .append_pair("s", &self.ssh_port.to_string())
            .append_pair("u", &self.ssh_user)
            .append_pair("f", &fp)
            .append_pair("k", &self.hpk)
            .append_pair("p", &self.port.to_string())
            .append_pair("t", &self.tok)
            .append_pair("e", &self.exp.to_string())
            .finish();
        format!("openmosh://pair?{query}")
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
