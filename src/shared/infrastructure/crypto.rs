//! Wire-contract primitives: X25519, HKDF-SHA256 (32-byte output),
//! XChaCha20-Poly1305. Secrets are held in `Zeroizing` buffers (control T-11).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("key agreement produced a non-contributory secret")]
    WeakKey,
    #[error("authenticated decryption failed")]
    Open,
}

pub type Key32 = Zeroizing<[u8; 32]>;

/// X25519 public key for a 32-byte secret (clamped per RFC 7748).
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    let s = StaticSecret::from(*secret);
    PublicKey::from(&s).to_bytes()
}

/// X25519 shared secret. Low-order peer points are rejected.
pub fn diffie_hellman(secret: &[u8; 32], peer: &[u8; 32]) -> Result<Key32, CryptoError> {
    let s = StaticSecret::from(*secret);
    let shared = s.diffie_hellman(&PublicKey::from(*peer));
    if !shared.was_contributory() {
        return Err(CryptoError::WeakKey);
    }
    Ok(Zeroizing::new(*shared.as_bytes()))
}

/// HKDF-SHA256 with a 32-byte output.
pub fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Key32 {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = Zeroizing::new([0u8; 32]);
    hk.expand(info, okm.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

/// XChaCha20-Poly1305 encryption.
pub fn seal(key: &[u8; 32], nonce: &[u8; 24], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory buffers")
}

/// XChaCha20-Poly1305 decryption.
pub fn open(
    key: &[u8; 32],
    nonce: &[u8; 24],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Open)
}

/// `N` bytes from the operating system CSPRNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dh_is_symmetric() {
        let a = [7u8; 32];
        let b = [9u8; 32];
        let ab = diffie_hellman(&a, &public_key(&b)).unwrap();
        let ba = diffie_hellman(&b, &public_key(&a)).unwrap();
        assert_eq!(*ab, *ba);
    }

    #[test]
    fn dh_rejects_low_order_point() {
        assert_eq!(
            diffie_hellman(&[7u8; 32], &[0u8; 32]),
            Err(CryptoError::WeakKey)
        );
    }

    #[test]
    fn seal_open_round_trip_and_aad_binding() {
        let key = [1u8; 32];
        let nonce = [2u8; 24];
        let ct = seal(&key, &nonce, b"hello", b"aad");
        assert_eq!(
            open(&key, &nonce, &ct, b"aad").unwrap().as_slice(),
            b"hello"
        );
        assert_eq!(open(&key, &nonce, &ct, b"other"), Err(CryptoError::Open));
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert_eq!(open(&key, &nonce, &bad, b"aad"), Err(CryptoError::Open));
    }

    #[test]
    fn hkdf_depends_on_salt_and_info() {
        let a = hkdf32(b"ikm", b"", b"x");
        let b = hkdf32(b"ikm", b"s", b"x");
        let c = hkdf32(b"ikm", b"", b"y");
        assert_ne!(*a, *b);
        assert_ne!(*a, *c);
    }
}
