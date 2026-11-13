//! A device's Web Push subscription (RFC 8030/8291/8292) and the validation
//! of what the app forwards for it. The VAPID private key is a secret: it is
//! never printed, logged or part of any view.

use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::features::push::domain::validate_webpush_endpoint;
use crate::shared::domain::encoding::b64u_decode_array;
use crate::shared::domain::error::ErrorCode;

/// Longest accepted VAPID `sub` claim.
pub const MAX_SUB_CHARS: usize = 256;

/// Web Push registration stored on a [`super::Device`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebPush {
    pub endpoint: String,
    /// b64u uncompressed P-256 point of the browser's subscription key.
    pub p256dh: String,
    /// b64u 16-byte subscription auth secret.
    pub auth: String,
    /// b64u uncompressed P-256 point of the VAPID key.
    pub vapid_pub: String,
    /// b64u 32-byte VAPID private scalar. Secret.
    pub vapid_priv: String,
    /// VAPID `sub` claim: `https://…` or `mailto:…`.
    pub sub: String,
    /// The push service answered `404`/`410`: the subscription is gone.
    #[serde(default)]
    pub stale: bool,
}

impl std::fmt::Debug for WebPush {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPush")
            .field("endpoint", &self.endpoint)
            .field("stale", &self.stale)
            .finish_non_exhaustive()
    }
}

/// What `devices set-webpush` reads on stdin (contract `v = 1`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebPushInput {
    pub v: u8,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub vapid_pub: String,
    pub vapid_priv: String,
    pub sub: String,
}

/// Why a Web Push registration was refused. Messages never echo the input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WebPushError {
    #[error("web push input is not the expected JSON object")]
    Malformed,
    #[error("unsupported web push version")]
    Version,
    #[error("web push endpoint is not allowed")]
    Endpoint,
    #[error("p256dh must be a 65-byte uncompressed P-256 point")]
    P256dh,
    #[error("auth must be 16 bytes")]
    Auth,
    #[error("vapid_pub must be a 65-byte uncompressed P-256 point")]
    VapidPub,
    #[error("vapid_priv must be a 32-byte P-256 scalar matching vapid_pub")]
    VapidPriv,
    #[error("sub must be an https:// URL or mailto: address of at most 256 characters")]
    Sub,
}

impl ErrorCode for WebPushError {
    fn code(&self) -> &'static str {
        "webpush_invalid"
    }
}

fn valid_point(b64: &str) -> Option<[u8; 65]> {
    let bytes = b64u_decode_array::<65>(b64)?;
    if bytes[0] != 0x04 {
        return None;
    }
    PublicKey::from_sec1_bytes(&*bytes).ok()?;
    Some(*bytes)
}

impl WebPushInput {
    pub fn parse(json: &str) -> Result<Self, WebPushError> {
        serde_json::from_str(json).map_err(|_| WebPushError::Malformed)
    }

    /// Checks every field and returns the record to store (not stale).
    pub fn validate(self) -> Result<WebPush, WebPushError> {
        if self.v != 1 {
            return Err(WebPushError::Version);
        }
        validate_webpush_endpoint(&self.endpoint).map_err(|_| WebPushError::Endpoint)?;
        valid_point(&self.p256dh).ok_or(WebPushError::P256dh)?;
        b64u_decode_array::<16>(&self.auth).ok_or(WebPushError::Auth)?;
        let vapid_pub = valid_point(&self.vapid_pub).ok_or(WebPushError::VapidPub)?;
        let scalar = b64u_decode_array::<32>(&self.vapid_priv).ok_or(WebPushError::VapidPriv)?;
        let secret = SecretKey::from_slice(&*scalar).map_err(|_| WebPushError::VapidPriv)?;
        if secret.public_key().to_sec1_point(false).as_bytes() != vapid_pub {
            return Err(WebPushError::VapidPriv);
        }
        let sub_ok = (self.sub.starts_with("https://") || self.sub.starts_with("mailto:"))
            && self.sub.chars().count() <= MAX_SUB_CHARS
            && !self.sub.chars().any(char::is_control);
        if !sub_ok {
            return Err(WebPushError::Sub);
        }
        Ok(WebPush {
            endpoint: self.endpoint,
            p256dh: self.p256dh,
            auth: self.auth,
            vapid_pub: self.vapid_pub,
            vapid_priv: self.vapid_priv,
            sub: self.sub,
            stale: false,
        })
    }
}

#[cfg(test)]
pub mod fixtures {
    use super::*;
    use crate::shared::domain::encoding::b64u_encode;

    /// A valid input built from fixed scalars.
    pub fn input(sub: &str) -> serde_json::Value {
        let point = |seed: u8| {
            let sk = SecretKey::from_slice(&[seed; 32]).unwrap();
            b64u_encode(sk.public_key().to_sec1_point(false).as_bytes())
        };
        serde_json::json!({
            "v": 1,
            "endpoint": "https://web.push.apple.com/abc",
            "p256dh": point(5),
            "auth": b64u_encode(&[7u8; 16]),
            "vapid_pub": point(9),
            "vapid_priv": b64u_encode(&[9u8; 32]),
            "sub": sub,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::input;
    use super::*;
    use crate::shared::domain::encoding::b64u_encode;

    fn check(v: serde_json::Value) -> Result<WebPush, WebPushError> {
        WebPushInput::parse(&v.to_string())?.validate()
    }

    #[test]
    fn a_valid_input_is_accepted_and_debug_hides_the_secret() {
        let w = check(input("mailto:me@example.com")).unwrap();
        assert!(!w.stale);
        let dbg = format!("{w:?}");
        assert!(!dbg.contains(&w.vapid_priv) && !dbg.contains(&w.auth));
        assert!(check(input("https://example.com/contact")).is_ok());
    }

    #[test]
    fn bad_fields_are_refused_with_one_code() {
        let ok = input("mailto:a@b.c");
        let with = |k: &str, v: serde_json::Value| {
            let mut x = ok.clone();
            x[k] = v;
            x
        };
        let cases = [
            (with("v", 2.into()), WebPushError::Version),
            (
                with("endpoint", "http://example.com/x".into()),
                WebPushError::Endpoint,
            ),
            (
                with("endpoint", "https://127.0.0.1/x".into()),
                WebPushError::Endpoint,
            ),
            (
                with("endpoint", "http://web.push.apple.com/x".into()),
                WebPushError::Endpoint,
            ),
            // http is fine for ntfy on a tailnet, never for Web Push.
            (
                with("endpoint", "http://100.101.1.2:2586/x".into()),
                WebPushError::Endpoint,
            ),
            (
                with("p256dh", b64u_encode(&[4u8; 65]).into()),
                WebPushError::P256dh,
            ),
            (
                with("p256dh", b64u_encode(&[4u8; 33]).into()),
                WebPushError::P256dh,
            ),
            (
                with("auth", b64u_encode(&[1u8; 15]).into()),
                WebPushError::Auth,
            ),
            (
                with("vapid_pub", b64u_encode(&[0u8; 65]).into()),
                WebPushError::VapidPub,
            ),
            (
                with("vapid_priv", b64u_encode(&[8u8; 32]).into()),
                WebPushError::VapidPriv,
            ),
            (
                with("vapid_priv", b64u_encode(&[9u8; 31]).into()),
                WebPushError::VapidPriv,
            ),
            (with("sub", "ftp://x".into()), WebPushError::Sub),
            (
                with("sub", format!("mailto:{}", "a".repeat(260)).into()),
                WebPushError::Sub,
            ),
        ];
        for (v, want) in cases {
            let got = check(v.clone()).unwrap_err();
            assert_eq!(got, want, "{v}");
            assert_eq!(got.code(), "webpush_invalid");
        }
        let mut extra = ok.clone();
        extra["extra"] = 1.into();
        assert_eq!(check(extra).unwrap_err(), WebPushError::Malformed);
        let mut missing = ok;
        missing.as_object_mut().unwrap().remove("sub");
        assert_eq!(check(missing).unwrap_err(), WebPushError::Malformed);
    }
}
