//! Actions, keymaps, text sanitization and errors.

use serde::Serialize;
use thiserror::Error;

use crate::features::agents::domain::{AgentStatus, PaneProbe};
use crate::features::events::domain::{Event, EventKind, Runtime, APPROVAL_TTL_SECS};
use crate::shared::domain::error::ErrorCode;
use crate::shared::domain::text::is_spoofing_char;

/// Maximum `respond --text` length in bytes, after sanitization.
pub const MAX_TEXT_BYTES: usize = 16384;

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
    #[error("text exceeds 16384 bytes")]
    TextTooLong,
    #[error("event has no pane to answer, or the pane is gone")]
    NoPane,
    #[error("the pane no longer shows this prompt (another process or a newer prompt is there)")]
    Stale,
    #[error("the approval is older than 15 minutes and can no longer be answered")]
    Expired,
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
            ResponseError::Stale => "stale",
            ResponseError::Expired => "expired",
            ResponseError::NoKeymap => "no_keymap",
            ResponseError::RuntimeUnavailable => "runtime_unavailable",
            ResponseError::Delivery => "runtime_error",
            ResponseError::Store => "store_error",
        }
    }
}

/// How far in the future an approval's `created_at` may be (clock skew)
/// before it is treated as unverifiable.
pub const APPROVAL_FUTURE_SKEW_SECS: u64 = 60;

/// An approval past its time to live cannot be answered. Neither can one dated
/// further in the future than [`APPROVAL_FUTURE_SKEW_SECS`] (the clock stepped
/// back), since its real age is unknown.
pub fn check_age(event: &Event, now: u64) -> Result<(), ResponseError> {
    if event.kind != EventKind::ApprovalRequired {
        return Ok(());
    }
    let too_old = now.saturating_sub(event.created_at) > APPROVAL_TTL_SECS;
    let from_future = event.created_at > now.saturating_add(APPROVAL_FUTURE_SKEW_SECS);
    if too_old || from_future {
        return Err(ResponseError::Expired);
    }
    Ok(())
}

/// Decides whether the pane, as probed right now, still shows the prompt the
/// event was recorded for. Anything that cannot be verified is refused.
///
/// - tmux has no agent model: the pane must still run the process recorded
///   with the event (`pane_pid`). Events without a recorded pid are refused.
/// - Herdr: the pane must still run the same agent, be `blocked` for an
///   approval (or waiting, not working, for a text answer), and still be in
///   the recorded blocked state (`state_change_seq`) when one was recorded.
pub fn check_target(event: &Event, probe: &PaneProbe) -> Result<(), ResponseError> {
    let PaneProbe::Live(now) = probe else {
        return Err(ResponseError::NoPane);
    };
    match event.runtime {
        Some(Runtime::Tmux) => match (event.context.pane_pid, now.pane_pid) {
            (Some(was), Some(is)) if was == is => Ok(()),
            _ => Err(ResponseError::Stale),
        },
        Some(Runtime::Herdr) => {
            if now.agent.as_deref() != Some(event.agent.as_str()) {
                return Err(ResponseError::Stale);
            }
            let waiting = match event.kind {
                EventKind::ApprovalRequired => now.status == Some(AgentStatus::Blocked),
                EventKind::Question | EventKind::Done => matches!(
                    now.status,
                    Some(AgentStatus::Blocked | AgentStatus::Idle | AgentStatus::Done)
                ),
            };
            if !waiting {
                return Err(ResponseError::Stale);
            }
            match (event.context.agent_seq, now.state_change_seq) {
                (Some(was), Some(is)) if was != is => Err(ResponseError::Stale),
                _ => Ok(()),
            }
        }
        None => Err(ResponseError::NoPane),
    }
}

/// Makes the answer one line (a newline would act as Enter in the agent's
/// prompt and submit early), removes other control, bidi, zero-width and
/// format characters and enforces the size limit (control T-9).
pub fn sanitize_text(text: &str) -> Result<String, ResponseError> {
    let clean: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !is_spoofing_char(*c))
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
    fn strips_bidi_zero_width_and_format_chars() {
        assert_eq!(
            sanitize_text("rm\u{202E}txt.exe\u{200B}\u{2066}x\u{FEFF}\u{00AD}\u{2028}y").unwrap(),
            "rmtxt.exex y"
        );
    }

    fn event(runtime: Runtime, kind: EventKind) -> Event {
        Event {
            id: "e".into(),
            seq: 1,
            source: "herdr".into(),
            agent: "claude".into(),
            kind,
            status: crate::features::events::domain::EventStatus::Pending,
            title: String::new(),
            summary: String::new(),
            pane: Some("p".into()),
            runtime: Some(runtime),
            created_at: 1000,
            resolved_at: None,
            resolution: None,
            context: Default::default(),
        }
    }

    fn herdr(agent: &str, status: AgentStatus, seq: u64) -> PaneProbe {
        PaneProbe::Live(crate::features::agents::domain::PaneState {
            pane_pid: None,
            agent: Some(agent.into()),
            status: Some(status),
            state_change_seq: Some(seq),
        })
    }

    fn tmux(pid: u32) -> PaneProbe {
        PaneProbe::Live(crate::features::agents::domain::PaneState {
            pane_pid: Some(pid),
            ..Default::default()
        })
    }

    #[test]
    fn approvals_expire_after_fifteen_minutes() {
        let e = event(Runtime::Herdr, EventKind::ApprovalRequired);
        assert_eq!(check_age(&e, 1000 + 900), Ok(()));
        assert_eq!(check_age(&e, 1000 + 901), Err(ResponseError::Expired));
        assert_eq!(ResponseError::Expired.code(), "expired");
        let q = event(Runtime::Herdr, EventKind::Question);
        assert_eq!(check_age(&q, 1000 + 5000), Ok(()));
    }

    #[test]
    fn approvals_dated_in_the_future_beyond_the_skew_are_refused() {
        // The host clock stepped back after the approval was recorded.
        let e = event(Runtime::Herdr, EventKind::ApprovalRequired);
        assert_eq!(check_age(&e, 1000 - 60), Ok(()));
        assert_eq!(check_age(&e, 1000 - 61), Err(ResponseError::Expired));
        assert_eq!(check_age(&e, 0), Err(ResponseError::Expired));
        // Questions never expire, whatever their timestamp.
        let q = event(Runtime::Herdr, EventKind::Question);
        assert_eq!(check_age(&q, 0), Ok(()));
    }

    #[test]
    fn tmux_target_must_keep_its_pane_pid() {
        let mut e = event(Runtime::Tmux, EventKind::ApprovalRequired);
        assert_eq!(
            check_target(&e, &tmux(7)),
            Err(ResponseError::Stale),
            "no pid recorded"
        );
        e.context.pane_pid = Some(7);
        assert_eq!(check_target(&e, &tmux(7)), Ok(()));
        assert_eq!(check_target(&e, &tmux(8)), Err(ResponseError::Stale));
        assert_eq!(
            check_target(&e, &PaneProbe::Gone),
            Err(ResponseError::NoPane)
        );
        assert_eq!(ResponseError::Stale.code(), "stale");
    }

    #[test]
    fn herdr_target_must_still_be_the_same_blocked_agent() {
        use AgentStatus::*;
        let mut e = event(Runtime::Herdr, EventKind::ApprovalRequired);
        // Old rows without a seq are answered only while still blocked.
        assert_eq!(check_target(&e, &herdr("claude", Blocked, 3)), Ok(()));
        assert_eq!(
            check_target(&e, &herdr("claude", Working, 3)),
            Err(ResponseError::Stale)
        );
        assert_eq!(
            check_target(&e, &herdr("codex", Blocked, 3)),
            Err(ResponseError::Stale)
        );
        e.context.agent_seq = Some(3);
        assert_eq!(check_target(&e, &herdr("claude", Blocked, 3)), Ok(()));
        assert_eq!(
            check_target(&e, &herdr("claude", Blocked, 5)),
            Err(ResponseError::Stale),
            "a newer prompt in the same pane"
        );
        let q = event(Runtime::Herdr, EventKind::Question);
        assert_eq!(check_target(&q, &herdr("claude", Idle, 9)), Ok(()));
        assert_eq!(
            check_target(&q, &herdr("claude", Working, 9)),
            Err(ResponseError::Stale)
        );
        assert_eq!(
            check_target(&q, &PaneProbe::Gone),
            Err(ResponseError::NoPane)
        );
    }

    #[test]
    fn enforces_limits() {
        assert_eq!(sanitize_text("\u{1b}\u{7}"), Err(ResponseError::EmptyText));
        assert_eq!(sanitize_text(" \n "), Err(ResponseError::EmptyText));
        assert!(sanitize_text(&"a".repeat(MAX_TEXT_BYTES)).is_ok());
        assert_eq!(
            sanitize_text(&"a".repeat(MAX_TEXT_BYTES + 1)),
            Err(ResponseError::TextTooLong)
        );
        // Control characters do not count toward the limit.
        assert!(sanitize_text(&format!(
            "{}{}",
            "a".repeat(MAX_TEXT_BYTES),
            "\u{1b}".repeat(10)
        ))
        .is_ok());
    }

    #[test]
    fn accepts_4000_multibyte_characters() {
        // The navigator allows 4000 characters; accented text is 2 bytes each.
        assert!(sanitize_text(&"é".repeat(4000)).is_ok());
        assert!(sanitize_text(&"🙂".repeat(4000)).is_ok());
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
