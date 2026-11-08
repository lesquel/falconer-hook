//! QR pairing encryption (wire contract §6.3, `v = 1`).
//!
//! ```text
//! k_req  = HKDF(ikm = X25519(eph, hpk), salt = token bytes, info = "openmosh/pair/v1/req")
//! k_resp = HKDF(ikm = X25519(eph, hpk), salt = token bytes, info = "openmosh/pair/v1/resp")
//! line   = {"v":1,"e":b64u(eph pub),"n":b64u(nonce),"c":b64u(AEAD(k_req, n, plaintext, aad))}
//! resp   = {"v":1,"n":b64u(nonce),"c":b64u(AEAD(k_resp, n, {"device_id","alias"}, aad))}
//! ```
//! The HKDF salt is the raw 16 token bytes (the QR carries them b64u-encoded).

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::features::pairing::domain::{
    PairCrypto, PairRequestPlain, PairResponsePlain, PairToken,
};
use crate::shared::domain::encoding::{b64u_decode, b64u_decode_array, b64u_encode};
use crate::shared::infrastructure::crypto::{
    diffie_hellman, hkdf32, open, random_bytes, seal, Key32,
};

pub const PAIR_AAD: &[u8] = b"openmosh/pair/v1";
pub const INFO_REQ: &[u8] = b"openmosh/pair/v1/req";
pub const INFO_RESP: &[u8] = b"openmosh/pair/v1/resp";
/// Maximum request line length, newline included.
pub const MAX_LINE_BYTES: usize = 16 * 1024;
/// Generic rejection, identical for every failure.
pub const REJECTED_LINE: &str = r#"{"v":1,"err":"pair_rejected"}"#;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestLine {
    v: u8,
    e: String,
    n: String,
    c: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseLine {
    v: u8,
    n: String,
    c: String,
}

pub struct PairKeys {
    pub k_req: Key32,
    pub k_resp: Key32,
}

pub fn derive_pair_keys(shared: &[u8; 32], token: &PairToken) -> PairKeys {
    PairKeys {
        k_req: hkdf32(shared, token, INFO_REQ),
        k_resp: hkdf32(shared, token, INFO_RESP),
    }
}

#[cfg(test)]
/// Phone side: builds a request line (no trailing newline). Used by the
/// shared test vectors and by tests that play the phone.
pub fn seal_request(
    eph_secret: &[u8; 32],
    hpk: &[u8; 32],
    token: &PairToken,
    nonce: &[u8; 24],
    plaintext: &str,
) -> Option<String> {
    let shared = diffie_hellman(eph_secret, hpk).ok()?;
    let keys = derive_pair_keys(&shared, token);
    let c = seal(&keys.k_req, nonce, plaintext.as_bytes(), PAIR_AAD);
    serde_json::to_string(&RequestLine {
        v: 1,
        e: b64u_encode(&crate::shared::infrastructure::crypto::public_key(
            eph_secret,
        )),
        n: b64u_encode(nonce),
        c: b64u_encode(&c),
    })
    .ok()
}

/// Host side: response line with an explicit nonce (no trailing newline).
pub fn seal_response_with_nonce(k_resp: &[u8; 32], nonce: &[u8; 24], plaintext: &str) -> String {
    let c = seal(k_resp, nonce, plaintext.as_bytes(), PAIR_AAD);
    serde_json::to_string(&ResponseLine {
        v: 1,
        n: b64u_encode(nonce),
        c: b64u_encode(&c),
    })
    .expect("response line serializes")
}

#[cfg(test)]
/// Phone side: decrypts a success response line.
pub fn open_response(k_resp: &[u8; 32], line: &str) -> Option<String> {
    let r: ResponseLine = serde_json::from_str(line.trim_end()).ok()?;
    if r.v != 1 {
        return None;
    }
    let n = b64u_decode_array::<24>(&r.n)?;
    let c = b64u_decode(&r.c)?;
    let pt = open(k_resp, &n, &c, PAIR_AAD).ok()?;
    String::from_utf8(pt.to_vec()).ok()
}

/// Host-side `PairCrypto` holding the host static secret.
pub struct X25519PairCrypto {
    host_secret: Zeroizing<[u8; 32]>,
}

impl X25519PairCrypto {
    pub fn new(host_secret: Zeroizing<[u8; 32]>) -> Self {
        Self { host_secret }
    }
}

impl PairCrypto for X25519PairCrypto {
    type Session = Key32;

    fn open_request(&self, line: &str, token: &PairToken) -> Option<(PairRequestPlain, Key32)> {
        if line.len() > MAX_LINE_BYTES {
            return None;
        }
        let req: RequestLine = serde_json::from_str(line.trim_end_matches(['\n', '\r'])).ok()?;
        if req.v != 1 {
            return None;
        }
        let e = b64u_decode_array::<32>(&req.e)?;
        let n = b64u_decode_array::<24>(&req.n)?;
        let c = b64u_decode(&req.c)?;
        let shared = diffie_hellman(&self.host_secret, &e).ok()?;
        let keys = derive_pair_keys(&shared, token);
        let pt = open(&keys.k_req, &n, &c, PAIR_AAD).ok()?;
        let plain: PairRequestPlain = serde_json::from_slice(&pt).ok()?;
        Some((plain, keys.k_resp))
    }

    fn seal_response(&self, session: &Key32, response: &PairResponsePlain) -> String {
        let json = Zeroizing::new(serde_json::to_string(response).expect("response serializes"));
        seal_response_with_nonce(session, &random_bytes::<24>(), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::infrastructure::crypto::public_key;

    fn plain(tok: &PairToken) -> String {
        format!(
            r#"{{"tok":"{}","name":"P","ssh_pub":"k","ppk":"p","endpoint":"https://e"}}"#,
            b64u_encode(tok)
        )
    }

    #[test]
    fn host_opens_what_phone_seals_and_phone_opens_reply() {
        let host = [5u8; 32];
        let tok = [6u8; 16];
        let line = seal_request(
            &[7u8; 32],
            &public_key(&host),
            &tok,
            &[8u8; 24],
            &plain(&tok),
        )
        .unwrap();
        let crypto = X25519PairCrypto::new(Zeroizing::new(host));
        let (req, k_resp) = crypto.open_request(&format!("{line}\n"), &tok).unwrap();
        assert_eq!(req.name, "P");
        let reply = crypto.seal_response(
            &k_resp,
            &PairResponsePlain {
                device_id: "d".into(),
                alias: "a".into(),
                ssh_user: "u".into(),
                ssh_port: 22,
                ssh_fp: "SHA256:f".into(),
            },
        );
        assert_eq!(
            open_response(&k_resp, &reply).unwrap(),
            r#"{"device_id":"d","alias":"a","ssh_user":"u","ssh_port":22,"ssh_fp":"SHA256:f"}"#
        );
    }

    #[test]
    fn wrong_token_or_tampering_fails() {
        let host = [5u8; 32];
        let tok = [6u8; 16];
        let line = seal_request(
            &[7u8; 32],
            &public_key(&host),
            &tok,
            &[8u8; 24],
            &plain(&tok),
        )
        .unwrap();
        let crypto = X25519PairCrypto::new(Zeroizing::new(host));
        assert!(crypto.open_request(&line, &[0u8; 16]).is_none());
        assert!(crypto
            .open_request(&line.replace("\"v\":1", "\"v\":2"), &tok)
            .is_none());
        assert!(crypto.open_request("not json", &tok).is_none());
        assert!(crypto
            .open_request(&"x".repeat(MAX_LINE_BYTES + 1), &tok)
            .is_none());
    }
}
