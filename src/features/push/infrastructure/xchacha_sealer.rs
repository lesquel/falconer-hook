//! Push encryption (wire contract §6.5, `v = 1`).
//!
//! ```text
//! k_push = HKDF(ikm = X25519(host_static, ppk), salt = "", info = "openmosh/push/v1/" + device_id)
//! body   = {"v":1,"kid":device_id,"n":b64u(nonce),"c":b64u(AEAD(k_push, n, plaintext, aad = "openmosh/push/v1"))}
//! ```

use zeroize::Zeroizing;

use crate::features::push::domain::{
    PushBody, PushError, PushPlaintext, PushTarget, Sealer, MAX_BODY_BYTES,
};
use crate::shared::domain::encoding::b64u_encode;
use crate::shared::infrastructure::crypto::{diffie_hellman, hkdf32, random_bytes, seal, Key32};

pub const PUSH_AAD: &[u8] = b"openmosh/push/v1";
pub const PUSH_INFO_PREFIX: &str = "openmosh/push/v1/";

pub fn push_key(
    host_secret: &[u8; 32],
    ppk: &[u8; 32],
    device_id: &str,
) -> Result<Key32, PushError> {
    let shared = diffie_hellman(host_secret, ppk).map_err(|_| PushError::BadKey)?;
    Ok(hkdf32(
        shared.as_slice(),
        b"",
        format!("{PUSH_INFO_PREFIX}{device_id}").as_bytes(),
    ))
}

/// Seals `plaintext` (already-serialized JSON) with an explicit nonce.
pub fn seal_push_with_nonce(
    host_secret: &[u8; 32],
    device_id: &str,
    ppk: &[u8; 32],
    nonce: &[u8; 24],
    plaintext: &str,
) -> Result<String, PushError> {
    let key = push_key(host_secret, ppk, device_id)?;
    let c = seal(&key, nonce, plaintext.as_bytes(), PUSH_AAD);
    let body = serde_json::to_string(&PushBody {
        v: 1,
        kid: device_id.to_string(),
        n: b64u_encode(nonce),
        c: b64u_encode(&c),
    })
    .expect("push body serializes");
    if body.len() > MAX_BODY_BYTES {
        return Err(PushError::TooLarge);
    }
    Ok(body)
}

pub struct XChaChaSealer {
    host_secret: Zeroizing<[u8; 32]>,
}

impl XChaChaSealer {
    pub fn new(host_secret: Zeroizing<[u8; 32]>) -> Self {
        Self { host_secret }
    }
}

impl Sealer for XChaChaSealer {
    fn seal(&self, target: &PushTarget, plaintext: &PushPlaintext) -> Result<String, PushError> {
        let json = Zeroizing::new(serde_json::to_string(plaintext).expect("plaintext serializes"));
        seal_push_with_nonce(
            &self.host_secret,
            &target.device_id,
            &target.ppk,
            &random_bytes::<24>(),
            &json,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::encoding::{b64u_decode, b64u_decode_array};
    use crate::shared::infrastructure::crypto::{open, public_key};

    #[test]
    fn device_decrypts_with_its_key_and_host_public() {
        let host = [3u8; 32];
        let device = [4u8; 32];
        let target = PushTarget {
            device_id: "dev1".into(),
            ppk: public_key(&device),
            endpoint: "https://e".into(),
        };
        let pt = PushPlaintext::new("e1", 1, "box", "claude", "done", "t", "s", "p", 5);
        let body = XChaChaSealer::new(Zeroizing::new(host))
            .seal(&target, &pt)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["kid"], "dev1");
        // Phone side: X25519(device, hpk).
        let k = push_key(&device, &public_key(&host), "dev1").unwrap();
        let n = b64u_decode_array::<24>(v["n"].as_str().unwrap()).unwrap();
        let c = b64u_decode(v["c"].as_str().unwrap()).unwrap();
        let out = open(&k, &n, &c, PUSH_AAD).unwrap();
        assert_eq!(
            out.as_slice(),
            serde_json::to_string(&pt).unwrap().as_bytes()
        );
        // A different device id derives a different key.
        let k2 = push_key(&device, &public_key(&host), "dev2").unwrap();
        assert!(open(&k2, &n, &c, PUSH_AAD).is_err());
    }

    #[test]
    fn oversized_body_is_refused() {
        let big = "x".repeat(4000);
        assert_eq!(
            seal_push_with_nonce(&[3; 32], "d", &public_key(&[4; 32]), &[0; 24], &big),
            Err(PushError::TooLarge)
        );
    }
}
