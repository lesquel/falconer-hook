//! Push wire model (plaintext and body), endpoint rules and ports.

mod endpoint;
mod webpush;

pub use endpoint::{forbidden_ip, validate_endpoint, validate_webpush_endpoint, EndpointError};
pub use webpush::{WebPushPayload, WebPushSender, WEBPUSH_PAD_BUCKETS, WEBPUSH_TTL_SECS};

use serde::Serialize;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

/// Maximum POST body size.
pub const MAX_BODY_BYTES: usize = 3 * 1024;
/// `summary` is truncated to this many characters before encryption.
pub const MAX_SUMMARY_CHARS: usize = 200;
/// `title` is truncated to this many characters before encryption.
pub const MAX_TITLE_CHARS: usize = 120;

/// Decrypted push content. Field order is the wire order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushPlaintext {
    pub id: String,
    pub seq: u64,
    pub host: String,
    pub agent: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub pane: String,
    pub ts: u64,
}

/// Plaintext sizes pushes are padded to, so the ciphertext length (visible to
/// the push relay) says little about the content. The largest bucket that
/// still fits [`MAX_BODY_BYTES`] once encrypted and base64-encoded is 2048.
pub const PAD_BUCKETS: &[usize] = &[512, 1024, 2048];

/// `json` followed by spaces up to the next bucket (JSON allows trailing
/// whitespace, so the phone parses it unchanged). Longer input is left as is.
pub fn pad_plaintext(json: &str) -> String {
    let target = PAD_BUCKETS
        .iter()
        .copied()
        .find(|b| *b >= json.len())
        .unwrap_or(json.len());
    let mut out = String::with_capacity(target);
    out.push_str(json);
    out.extend(std::iter::repeat_n(' ', target - json.len()));
    out
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

impl PushPlaintext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &str,
        seq: u64,
        host: &str,
        agent: &str,
        kind: &str,
        title: &str,
        summary: &str,
        pane: &str,
        ts: u64,
    ) -> Self {
        Self {
            id: id.into(),
            seq,
            host: host.into(),
            agent: agent.into(),
            kind: kind.into(),
            title: truncate_chars(title, MAX_TITLE_CHARS),
            summary: truncate_chars(summary, MAX_SUMMARY_CHARS),
            pane: pane.into(),
            ts,
        }
    }
}

/// Encrypted body POSTed to the endpoint. Field order is the wire order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushBody {
    pub v: u8,
    pub kid: String,
    pub n: String,
    pub c: String,
}

/// One device that receives pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub device_id: String,
    pub ppk: [u8; 32],
    pub endpoint: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PushError {
    #[error("push body exceeds 3 KiB")]
    TooLarge,
    #[error("device push key is invalid")]
    BadKey,
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
}

impl ErrorCode for PushError {
    fn code(&self) -> &'static str {
        match self {
            PushError::TooLarge => "push_too_large",
            PushError::BadKey => "invalid_key",
            PushError::Endpoint(_) => "invalid_endpoint",
        }
    }
}

/// Seals a plaintext for one device and returns the JSON body string.
pub trait Sealer {
    fn seal(&self, target: &PushTarget, plaintext: &PushPlaintext) -> Result<String, PushError>;
}

/// Result of one POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostOutcome {
    Delivered,
    /// `404`/`410`: the endpoint no longer exists.
    Stale,
    Failed(&'static str),
}

/// Delivers a body to an endpoint.
pub trait PushPublisher {
    fn post(&self, endpoint: &str, body: &str) -> PostOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_serializes_in_wire_order_and_truncates() {
        let long = "é".repeat(300);
        let p = PushPlaintext::new("id", 3, "box", "claude", "done", "t", &long, "w1:p1", 9);
        assert_eq!(p.summary.chars().count(), 200);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.starts_with(r#"{"id":"id","seq":3,"host":"box","agent":"claude","kind":"done","title":"t","summary":""#));
        assert!(json.ends_with(r#","pane":"w1:p1","ts":9}"#));
    }
}
