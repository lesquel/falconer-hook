//! Web Push wire payload and port. The notification is shown by the browser
//! on the lock screen, so it carries no host, agent, command, summary or pane.

use serde::Serialize;

use super::PostOutcome;
use crate::features::pairing::domain::WebPush;

/// Plaintext sizes the Web Push payload is padded to (record bytes, including
/// the `0x02` delimiter). A longer record is left unpadded.
pub const WEBPUSH_PAD_BUCKETS: &[usize] = &[256, 512];
/// `TTL` header: an approval is stale after 15 minutes.
pub const WEBPUSH_TTL_SECS: u32 = 900;

/// Decrypted Web Push content. Field order is the wire order.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct WebPushPayload {
    pub v: u8,
    pub kind: String,
    pub title: &'static str,
    pub body: &'static str,
    pub url: &'static str,
}

/// Generic notification title for an event kind.
pub fn generic_title(kind: &str) -> &'static str {
    match kind {
        "approval_required" | "approval" | "permission" => "An agent needs approval",
        "question" => "An agent has a question",
        "done" | "finished" => "An agent finished",
        _ => "Falconer",
    }
}

impl WebPushPayload {
    pub fn for_kind(kind: &str) -> Self {
        Self {
            v: 1,
            kind: kind.to_string(),
            title: generic_title(kind),
            body: "Open Falconer to respond.",
            url: "openmosh:///inbox",
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("web push payload serializes")
    }
}

/// Encrypts, signs and delivers one Web Push message for `kind`.
pub trait WebPushSender {
    fn send(&self, subscription: &WebPush, kind: &str) -> PostOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_generic_and_in_wire_order() {
        assert_eq!(
            WebPushPayload::for_kind("approval_required").to_json(),
            r#"{"v":1,"kind":"approval_required","title":"An agent needs approval","body":"Open Falconer to respond.","url":"openmosh:///inbox"}"#
        );
        assert_eq!(generic_title("question"), "An agent has a question");
        assert_eq!(generic_title("done"), "An agent finished");
        assert_eq!(generic_title("resolved"), "Falconer");
    }
}
