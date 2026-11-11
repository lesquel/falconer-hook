//! Web Push message encryption: RFC 8291 over the RFC 8188 `aes128gcm`
//! content coding, single record.
//!
//! ```text
//! ecdh     = ECDH(as_private, ua_public)
//! IKM      = HKDF(salt = auth, ikm = ecdh, info = "WebPush: info\0" || ua_public || as_public, 32)
//! CEK      = HKDF(salt = salt, ikm = IKM, info = "Content-Encoding: aes128gcm\0", 16)
//! NONCE    = HKDF(salt = salt, ikm = IKM, info = "Content-Encoding: nonce\0", 12)
//! body     = salt(16) || rs(4096, u32 BE) || idlen(65) || as_public(65) || AES-128-GCM(CEK, NONCE, data || 0x02 || 0x00…)
//! ```

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{ecdh::diffie_hellman, PublicKey, SecretKey};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::features::push::domain::{PushError, WEBPUSH_PAD_BUCKETS};
use crate::shared::infrastructure::crypto::random_bytes;

/// Record size announced in the header.
pub const RECORD_SIZE: u32 = 4096;
const DELIMITER: u8 = 0x02;

/// Length of the RFC 8188 header for a 65-byte key id.
pub const HEADER_LEN: usize = 16 + 4 + 1 + 65;

/// Total record data length (plaintext, delimiter, zero padding) for `len`
/// plaintext bytes: the smallest bucket that fits, or no padding beyond the
/// largest bucket.
pub fn padded_len(len: usize) -> usize {
    WEBPUSH_PAD_BUCKETS
        .iter()
        .copied()
        .find(|b| *b > len)
        .unwrap_or(len + 1)
}

/// Encrypts with an explicit sender key and salt (test vectors) padding the
/// record data to `pad_to` bytes; `pad_to` below `plaintext.len() + 1` means
/// just the delimiter.
pub fn encrypt_with(
    ua_public: &[u8; 65],
    auth: &[u8; 16],
    plaintext: &[u8],
    as_secret: &SecretKey,
    salt: &[u8; 16],
    pad_to: usize,
) -> Result<Vec<u8>, PushError> {
    let ua = PublicKey::from_sec1_bytes(ua_public).map_err(|_| PushError::BadKey)?;
    let as_public = as_secret.public_key().to_encoded_point(false);
    let as_public = as_public.as_bytes();
    let shared = diffie_hellman(as_secret.to_nonzero_scalar(), ua.as_affine());

    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, ikm.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 output length");

    let hk = Hkdf::<Sha256>::new(Some(salt), ikm.as_ref());
    let mut cek = Zeroizing::new([0u8; 16]);
    hk.expand(b"Content-Encoding: aes128gcm\0", cek.as_mut())
        .expect("16 bytes is a valid HKDF-SHA256 output length");
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .expect("12 bytes is a valid HKDF-SHA256 output length");

    let mut data = Zeroizing::new(Vec::with_capacity(pad_to.max(plaintext.len() + 1)));
    data.extend_from_slice(plaintext);
    data.push(DELIMITER);
    if data.len() < pad_to {
        data.resize(pad_to, 0);
    }
    let ct = Aes128Gcm::new(cek.as_ref().into())
        .encrypt(Nonce::from_slice(&nonce), data.as_slice())
        .expect("AES-128-GCM encryption cannot fail for in-memory buffers");

    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(65);
    out.extend_from_slice(as_public);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Encrypts `plaintext` for a subscription with a fresh sender key and salt,
/// padded to the bucket sizes of [`padded_len`].
pub fn seal(ua_public: &[u8; 65], auth: &[u8; 16], plaintext: &[u8]) -> Result<Vec<u8>, PushError> {
    let as_secret = SecretKey::random(&mut rand_core::OsRng);
    let salt = random_bytes::<16>();
    encrypt_with(
        ua_public,
        auth,
        plaintext,
        &as_secret,
        &salt,
        padded_len(plaintext.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::encoding::b64u_decode;

    fn b64(s: &str) -> Vec<u8> {
        b64u_decode(&s.split_whitespace().collect::<String>()).unwrap()
    }

    /// Test-only receiver: what the browser does with a body.
    fn decrypt(ua_secret: &SecretKey, auth: &[u8; 16], body: &[u8]) -> Vec<u8> {
        let salt: [u8; 16] = body[..16].try_into().unwrap();
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
        assert_eq!(body[20], 65);
        let as_public = &body[21..HEADER_LEN];
        let ua_public = ua_secret.public_key().to_encoded_point(false);
        let as_pub = PublicKey::from_sec1_bytes(as_public).unwrap();
        let shared = diffie_hellman(ua_secret.to_nonzero_scalar(), as_pub.as_affine());
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(ua_public.as_bytes());
        info.extend_from_slice(as_public);
        let mut ikm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
            .expand(&info, &mut ikm)
            .unwrap();
        let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
        let mut cek = [0u8; 16];
        hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
            .unwrap();
        let mut nonce = [0u8; 12];
        hk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();
        let data = Aes128Gcm::new(&cek.into())
            .decrypt(Nonce::from_slice(&nonce), &body[HEADER_LEN..])
            .unwrap();
        let end = data.iter().rposition(|b| *b != 0).unwrap();
        assert_eq!(data[end], DELIMITER, "last non-zero byte is the delimiter");
        data[..end].to_vec()
    }

    #[test]
    fn matches_the_rfc_8291_appendix_a_vector() {
        let plaintext = b64("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        let as_secret =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public: [u8; 65] = b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcx aOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        )
        .try_into()
        .unwrap();
        let auth: [u8; 16] = b64("BTBZMqHH6r4Tts7J_aSIgg").try_into().unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();

        let body = encrypt_with(
            &ua_public,
            &auth,
            &plaintext,
            &as_secret,
            &salt,
            plaintext.len() + 1,
        )
        .unwrap();

        let header = b64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z 9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
             mlMoZIIgDll6e3vCYLocInmYWAmS6Tlz AC8wEqKK6PBru3jl7A8",
        );
        let ciphertext = b64(
            "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrd irN6GcG7sFz1y1sqLgVi1VhjVkHsUoEs
             bI_0LpXMuGvnzQ",
        );
        assert_eq!(header.len(), 86);
        assert_eq!(&body[..HEADER_LEN], header.as_slice());
        assert_eq!(&body[HEADER_LEN..], ciphertext.as_slice());
    }

    #[test]
    fn round_trips_and_pads_to_fixed_buckets() {
        let ua = SecretKey::random(&mut rand_core::OsRng);
        let ua_public: [u8; 65] = ua
            .public_key()
            .to_encoded_point(false)
            .as_bytes()
            .try_into()
            .unwrap();
        let auth = [3u8; 16];
        let short = seal(&ua_public, &auth, b"{\"v\":1}").unwrap();
        let longer = seal(&ua_public, &auth, &[b'x'; 200]).unwrap();
        // header + (256-byte bucket + 16-byte GCM tag)
        assert_eq!(short.len(), HEADER_LEN + 256 + 16);
        assert_eq!(short.len(), longer.len(), "same size for different content");
        assert_eq!(decrypt(&ua, &auth, &short), b"{\"v\":1}");
        assert_eq!(decrypt(&ua, &auth, &longer), vec![b'x'; 200]);
        let mid = seal(&ua_public, &auth, &[b'y'; 300]).unwrap();
        assert_eq!(mid.len(), HEADER_LEN + 512 + 16);
        // Beyond the largest bucket: delimiter only.
        let big = seal(&ua_public, &auth, &[b'z'; 600]).unwrap();
        assert_eq!(big.len(), HEADER_LEN + 601 + 16);
        assert_eq!(decrypt(&ua, &auth, &big).len(), 600);
        // Fresh salt and sender key every time.
        assert_ne!(short, seal(&ua_public, &auth, b"{\"v\":1}").unwrap());
        // A wrong auth secret does not decrypt.
        let r = std::panic::catch_unwind(|| decrypt(&ua, &[4u8; 16], &short));
        assert!(r.is_err());
    }

    #[test]
    fn a_bad_subscription_key_is_refused() {
        let sk = SecretKey::random(&mut rand_core::OsRng);
        assert_eq!(
            encrypt_with(&[4u8; 65], &[0; 16], b"x", &sk, &[0; 16], 256),
            Err(PushError::BadKey)
        );
    }
}
