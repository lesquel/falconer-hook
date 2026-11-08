//! Wire encodings. `base64` is a pure, I/O-free crate and is the one documented
//! exception to the "std + serde + thiserror" rule for `domain` modules.

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine;

/// base64url without padding (`b64u` in the wire contract).
pub fn b64u_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Strict b64u decode: rejects padding and the standard alphabet.
pub fn b64u_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.as_bytes()).ok()
}

/// Decodes a b64u string that must be exactly `N` bytes long.
pub fn b64u_decode_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    let v = b64u_decode(s)?;
    v.try_into().ok()
}

/// Standard base64 without padding, as printed by `ssh-keygen -lf`.
pub fn b64_std_nopad(bytes: &[u8]) -> String {
    STANDARD_NO_PAD.encode(bytes)
}

/// Standard base64 decode, padding required as in OpenSSH public keys.
pub fn b64_std_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .ok()
}

/// Constant-time byte comparison for tokens and MACs.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64u_round_trip_without_padding() {
        let s = b64u_encode(&[0xfb, 0xff, 0x00]);
        assert_eq!(s, "-_8A");
        assert_eq!(b64u_decode(&s).unwrap(), vec![0xfb, 0xff, 0x00]);
        assert!(b64u_decode("-_8A=").is_none(), "padding must be rejected");
        assert!(
            b64u_decode("+/8A").is_none(),
            "standard alphabet must be rejected"
        );
    }

    #[test]
    fn decode_array_checks_length() {
        assert!(b64u_decode_array::<2>(&b64u_encode(&[1, 2, 3])).is_none());
        assert_eq!(
            b64u_decode_array::<3>(&b64u_encode(&[1, 2, 3])),
            Some([1, 2, 3])
        );
    }

    #[test]
    fn ct_eq_compares_content_and_length() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
