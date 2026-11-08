//! Actions, keymaps, text sanitization and errors.

use serde::Serialize;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

/// Maximum `respond --text` length in bytes, after sanitization.
pub const MAX_TEXT_BYTES: usize = 4096;

/// Key names a keymap may use (mapped per runtime by the adapters).
pub const KNOWN_KEYS: &[&str] = &[
    "enter",
    "esc",
    "tab",
    "space",
    "backspace",
    "up",
    "down",
    "left",
    "right",
    "y",
    "n",
    "1",
    "2",
    "3",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Deny,
    Text,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Allow => "allow",
            Action::Deny => "deny",
            Action::Text => "text",
        }
    }
}

/// Per-agent keystrokes (versioned data in `data/keymaps/*.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub agent: String,
    pub version: u32,
    pub verified: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub submit: Vec<String>,
}

impl Keymap {
    /// Every key must be a known name and allow/deny must not be empty.
    pub fn is_valid(&self) -> bool {
        let known = |ks: &[String]| ks.iter().all(|k| KNOWN_KEYS.contains(&k.as_str()));
        !self.allow.is_empty()
            && !self.deny.is_empty()
            && known(&self.allow)
            && known(&self.deny)
            && known(&self.submit)
    }
}

/// Looks up the keymap for an agent name.
pub trait KeymapSource {
    fn keymap(&self, agent: &str) -> Option<Keymap>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResponseError {
    #[error("event not found")]
    NotFound,
    #[error("event is already resolved")]
    AlreadyResolved,
    #[error("this action does not apply to the event")]
    InvalidAction,
    #[error("--text is required for the text action and only allowed with it")]
    TextArgument,
    #[error("text is empty after removing control characters")]
    EmptyText,
    #[error("text exceeds 4096 bytes")]
    TextTooLong,
    #[error("event has no pane to answer")]
    NoPane,
    #[error("no keymap for this agent")]
    NoKeymap,
    #[error("the agent runtime is not available")]
    RuntimeUnavailable,
    #[error("delivering the keystrokes failed")]
    Delivery,
    #[error("event store failure")]
    Store,
}

impl ErrorCode for ResponseError {
    fn code(&self) -> &'static str {
        match self {
            ResponseError::NotFound => "not_found",
            ResponseError::AlreadyResolved => "already_resolved",
            ResponseError::InvalidAction | ResponseError::TextArgument => "invalid_input",
            ResponseError::EmptyText => "invalid_input",
            ResponseError::TextTooLong => "text_too_long",
            ResponseError::NoPane => "no_pane",
            ResponseError::NoKeymap => "no_keymap",
            ResponseError::RuntimeUnavailable => "runtime_unavailable",
            ResponseError::Delivery => "runtime_error",
            ResponseError::Store => "store_error",
        }
    }
}

/// Makes the answer one line (a newline would act as Enter in the agent's
/// prompt and submit early), removes other control characters and enforces
/// the size limit (control T-9).
pub fn sanitize_text(text: &str) -> Result<String, ResponseError> {
    let clean: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    if clean.trim().is_empty() {
        return Err(ResponseError::EmptyText);
    }
    if clean.len() > MAX_TEXT_BYTES {
        return Err(ResponseError::TextTooLong);
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_control_chars_and_turns_newlines_into_spaces() {
        assert_eq!(
            sanitize_text("ok\u{1b}[31m\r\nnext\u{7}\t\u{9b}x").unwrap(),
            "ok[31m nextx"
        );
    }

    #[test]
    fn enforces_limits() {
        assert_eq!(sanitize_text("\u{1b}\u{7}"), Err(ResponseError::EmptyText));
        assert_eq!(sanitize_text(" \n "), Err(ResponseError::EmptyText));
        assert!(sanitize_text(&"a".repeat(4096)).is_ok());
        assert_eq!(
            sanitize_text(&"a".repeat(4097)),
            Err(ResponseError::TextTooLong)
        );
        // Control characters do not count toward the limit.
        assert!(sanitize_text(&format!("{}{}", "a".repeat(4096), "\u{1b}".repeat(10))).is_ok());
    }

    #[test]
    fn keymap_validation() {
        let mut k = Keymap {
            agent: "claude".into(),
            version: 1,
            verified: false,
            allow: vec!["enter".into()],
            deny: vec!["esc".into()],
            submit: vec!["enter".into()],
        };
        assert!(k.is_valid());
        k.deny = vec!["C-c".into()];
        assert!(!k.is_valid());
        k.deny = vec![];
        assert!(!k.is_valid());
    }
}
