//! VAPID (RFC 8292): the `Authorization: vapid t=<JWT>, k=<public key>`
//! header proving who sends to a push service. The JWT is ES256 over
//! `b64u(header).b64u(claims)`, signature as raw `r || s`.

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde_json::json;
use url::Url;

use crate::features::pairing::domain::WebPush;
use crate::shared::domain::encoding::{b64u_decode_array, b64u_encode};

/// Lifetime of a token (the RFC allows at most 24 h).
pub const TOKEN_TTL_SECS: u64 = 12 * 60 * 60;

/// `scheme://host[:port]` of the push service, port only when not the default.
pub fn audience(endpoint: &str) -> Option<String> {
    let url = Url::parse(endpoint).ok()?;
    url.host()?;
    Some(url.origin().ascii_serialization())
}

/// The signed JWT for `wp.endpoint`, valid until `now + 12 h`.
pub fn jwt(wp: &WebPush, now: u64) -> Option<String> {
    let aud = audience(&wp.endpoint)?;
    let key = b64u_decode_array::<32>(&wp.vapid_priv)?;
    let key = SigningKey::from_slice(key.as_ref()).ok()?;
    let claims = json!({"aud": aud, "exp": now + TOKEN_TTL_SECS, "sub": wp.sub});
    let signing_input = format!(
        "{}.{}",
        b64u_encode(br#"{"typ":"JWT","alg":"ES256"}"#),
        b64u_encode(claims.to_string().as_bytes())
    );
    let sig: Signature = key.sign(signing_input.as_bytes());
    Some(format!("{signing_input}.{}", b64u_encode(&sig.to_bytes())))
}

/// The `Authorization` header value.
pub fn authorization(wp: &WebPush, now: u64) -> Option<String> {
    Some(format!("vapid t={}, k={}", jwt(wp, now)?, wp.vapid_pub))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pairing::domain::WebPushInput;
    use crate::shared::domain::encoding::b64u_decode;
    use p256::ecdsa::{signature::Verifier, VerifyingKey};

    fn wp(endpoint: &str) -> WebPush {
        let mut v =
            crate::features::pairing::domain::webpush_fixtures::input("mailto:me@example.com");
        v["endpoint"] = endpoint.into();
        WebPushInput::parse(&v.to_string())
            .unwrap()
            .validate()
            .unwrap()
    }

    #[test]
    fn audience_is_the_origin_with_a_port_only_when_not_default() {
        assert_eq!(
            audience("https://web.push.apple.com/abc/def").as_deref(),
            Some("https://web.push.apple.com")
        );
        assert_eq!(
            audience("https://push.example.com:8443/x?y=1").as_deref(),
            Some("https://push.example.com:8443")
        );
        assert_eq!(
            audience("https://push.example.com:443/x").as_deref(),
            Some("https://push.example.com")
        );
        assert_eq!(audience("not a url"), None);
    }

    #[test]
    fn the_jwt_verifies_with_the_public_key_and_has_the_right_claims() {
        let w = wp("https://fcm.googleapis.com:443/fcm/send/xyz");
        let header = authorization(&w, 1_000).unwrap();
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (token, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, w.vapid_pub);

        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(
            b64u_decode(parts[0]).unwrap(),
            br#"{"typ":"JWT","alg":"ES256"}"#
        );
        let claims: serde_json::Value =
            serde_json::from_slice(&b64u_decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_000 + 43_200);
        assert_eq!(claims["sub"], "mailto:me@example.com");

        let sig_bytes = b64u_decode(parts[2]).unwrap();
        assert_eq!(sig_bytes.len(), 64, "raw r||s");
        let pubkey = VerifyingKey::from_sec1_bytes(&b64u_decode(&w.vapid_pub).unwrap()).unwrap();
        let sig = Signature::from_slice(&sig_bytes).unwrap();
        pubkey
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
        // A tampered claim set does not verify.
        assert!(pubkey
            .verify(format!("{}.{}", parts[0], "e30").as_bytes(), &sig)
            .is_err());
    }
}
