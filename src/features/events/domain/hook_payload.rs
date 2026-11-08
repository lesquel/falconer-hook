//! Maps an agent hook payload (JSON on stdin of `emit`) to a `NewEvent`.
//!
//! Supported shapes:
//! - Claude Code hooks: `hook_event_name` = `Notification` | `PermissionRequest` | `Stop`.
//! - Generic: `{"kind":"approval_required|question|done","title":"…","summary":"…"}`.
//!
//! - Any payload with a kind fixed by the caller (`emit --kind`), used by
//!   agents whose hook payloads have no shared shape (see [`parse_with_kind`]).
//!
//! Only `message`/`title`/`summary` (or the agent's last-message field) are kept; transcript paths, session ids
//! and tool inputs are ignored.

use serde_json::Value;
use thiserror::Error;

use super::{EventKind, NewEvent, PaneRef};
use crate::shared::domain::error::ErrorCode;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PayloadError {
    #[error("event payload is not valid JSON")]
    Json,
    #[error("unsupported event payload")]
    Unsupported,
    #[error("invalid source name")]
    Source,
}

impl ErrorCode for PayloadError {
    fn code(&self) -> &'static str {
        "invalid_input"
    }
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

pub fn valid_source(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn parse_hook_payload(
    source: &str,
    raw: &str,
    pane: Option<PaneRef>,
    now: u64,
) -> Result<NewEvent, PayloadError> {
    if !valid_source(source) {
        return Err(PayloadError::Source);
    }
    let v: Value = serde_json::from_str(raw).map_err(|_| PayloadError::Json)?;
    let agent = source.to_string();
    let (kind, title, summary) = match str_field(&v, "hook_event_name") {
        Some("Notification") => {
            let message = str_field(&v, "message").unwrap_or_default();
            let permission = str_field(&v, "notification_type") == Some("permission_prompt")
                || message.to_ascii_lowercase().contains("permission");
            if permission {
                (
                    EventKind::ApprovalRequired,
                    format!("{agent} needs approval"),
                    message.to_string(),
                )
            } else {
                (
                    EventKind::Question,
                    format!("{agent} is waiting for input"),
                    message.to_string(),
                )
            }
        }
        Some("PermissionRequest") => {
            let tool = str_field(&v, "tool_name").unwrap_or("a tool");
            (
                EventKind::ApprovalRequired,
                format!("{agent} needs approval"),
                format!("Allow {tool}?"),
            )
        }
        Some("Stop") => (EventKind::Done, format!("{agent} finished"), String::new()),
        Some(_) => return Err(PayloadError::Unsupported),
        None => {
            let kind = str_field(&v, "kind")
                .and_then(EventKind::parse)
                .ok_or(PayloadError::Unsupported)?;
            let title = str_field(&v, "title")
                .map(str::to_string)
                .unwrap_or_else(|| format!("{agent}: {}", kind.as_str()));
            (
                kind,
                title,
                str_field(&v, "summary").unwrap_or_default().to_string(),
            )
        }
    };
    Ok(finish(source, kind, title, summary, pane, now))
}

/// Event kind chosen by the hook command line (`emit --kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    Fixed(EventKind),
    /// A notification: approval when it is about a permission, else a question.
    Notification,
}

/// Fields that carry the human message, in order of preference, across
/// Codex (`last_assistant_message`), Gemini (`prompt_response`), Cursor
/// (`text`), Copilot (`message`) and OpenCode plugin payloads.
const MESSAGE_FIELDS: &[&str] = &[
    "message",
    "summary",
    "last_assistant_message",
    "prompt_response",
    "text",
];

fn is_permission(v: &Value) -> bool {
    let has = |k: &str, needle: &str| {
        str_field(v, k).is_some_and(|s| s.to_ascii_lowercase().contains(needle))
    };
    has("notification_type", "permission")
        || has("message", "permission")
        || has("message", "approval")
}

/// Builds an event whose kind is fixed by the caller. The payload is optional:
/// empty or non-JSON input still records the event, without a summary.
pub fn parse_with_kind(
    source: &str,
    kind: EmitKind,
    raw: &str,
    pane: Option<PaneRef>,
    now: u64,
) -> Result<NewEvent, PayloadError> {
    if !valid_source(source) {
        return Err(PayloadError::Source);
    }
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let kind = match kind {
        EmitKind::Fixed(k) => k,
        EmitKind::Notification if is_permission(&v) => EventKind::ApprovalRequired,
        EmitKind::Notification => EventKind::Question,
    };
    let title = match kind {
        EventKind::ApprovalRequired => format!("{source} needs approval"),
        EventKind::Question => format!("{source} is waiting for input"),
        EventKind::Done => format!("{source} finished"),
    };
    let summary = MESSAGE_FIELDS
        .iter()
        .find_map(|k| str_field(&v, k).filter(|s| !s.trim().is_empty()))
        .unwrap_or_default()
        .to_string();
    Ok(finish(source, kind, title, summary, pane, now))
}

fn finish(
    source: &str,
    kind: EventKind,
    title: String,
    summary: String,
    pane: Option<PaneRef>,
    now: u64,
) -> NewEvent {
    let clean = |s: String| s.chars().filter(|c| !c.is_control()).collect::<String>();
    NewEvent {
        source: source.to_string(),
        agent: source.to_string(),
        kind,
        title: clean(title).chars().take(120).collect(),
        summary: clean(summary).chars().take(200).collect(),
        pane,
        created_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::events::domain::Runtime;

    fn pane() -> Option<PaneRef> {
        Some(PaneRef {
            runtime: Runtime::Tmux,
            pane: "%3".into(),
        })
    }

    #[test]
    fn claude_permission_notification_is_approval() {
        let raw = r#"{"session_id":"s","transcript_path":"/secret/t.jsonl","cwd":"/x","hook_event_name":"Notification","message":"Claude needs your permission to use Bash"}"#;
        let e = parse_hook_payload("claude", raw, pane(), 7).unwrap();
        assert_eq!(e.kind, EventKind::ApprovalRequired);
        assert_eq!(e.title, "claude needs approval");
        assert_eq!(e.summary, "Claude needs your permission to use Bash");
        assert_eq!(e.pane, pane());
        assert_eq!(e.created_at, 7);
    }

    #[test]
    fn claude_idle_notification_is_question_and_stop_is_done() {
        let q = parse_hook_payload(
            "claude",
            r#"{"hook_event_name":"Notification","message":"Claude is waiting for your input"}"#,
            None,
            1,
        )
        .unwrap();
        assert_eq!(q.kind, EventKind::Question);
        let d = parse_hook_payload(
            "claude",
            r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
            None,
            1,
        )
        .unwrap();
        assert_eq!(d.kind, EventKind::Done);
        assert_eq!(d.title, "claude finished");
        let p = parse_hook_payload("claude", r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}"#, None, 1).unwrap();
        assert_eq!(p.kind, EventKind::ApprovalRequired);
        assert_eq!(p.summary, "Allow Bash?");
    }

    #[test]
    fn generic_payload_and_control_chars() {
        let e = parse_hook_payload(
            "codex",
            r#"{"kind":"question","title":"t\u001b[2J","summary":"s"}"#,
            None,
            1,
        )
        .unwrap();
        assert_eq!(e.kind, EventKind::Question);
        assert_eq!(e.title, "t[2J");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            parse_hook_payload("claude", "nope", None, 1),
            Err(PayloadError::Json)
        );
        assert_eq!(
            parse_hook_payload("claude", r#"{"hook_event_name":"PreToolUse"}"#, None, 1),
            Err(PayloadError::Unsupported)
        );
        assert_eq!(
            parse_hook_payload("claude", r#"{"kind":"weird"}"#, None, 1),
            Err(PayloadError::Unsupported)
        );
        assert_eq!(
            parse_hook_payload("bad name", "{}", None, 1),
            Err(PayloadError::Source)
        );
    }

    #[test]
    fn fixed_kind_accepts_any_payload_and_picks_the_message_field() {
        let done = parse_with_kind(
            "codex",
            EmitKind::Fixed(EventKind::Done),
            r#"{"hook_event_name":"Stop","last_assistant_message":"All tests pass\u001b"}"#,
            None,
            3,
        )
        .unwrap();
        assert_eq!(done.kind, EventKind::Done);
        assert_eq!(done.title, "codex finished");
        assert_eq!(done.summary, "All tests pass");
        assert_eq!(done.agent, "codex");
        let empty =
            parse_with_kind("aider", EmitKind::Fixed(EventKind::Done), "", None, 3).unwrap();
        assert_eq!(empty.summary, "");
        let junk = parse_with_kind(
            "kiro",
            EmitKind::Fixed(EventKind::Question),
            "not json",
            None,
            3,
        )
        .unwrap();
        assert_eq!(junk.title, "kiro is waiting for input");
        assert_eq!(
            parse_with_kind("bad name", EmitKind::Notification, "{}", None, 1),
            Err(PayloadError::Source)
        );
    }

    #[test]
    fn notification_kind_detects_permission_prompts() {
        let gemini = r#"{"hook_event_name":"Notification","notification_type":"ToolPermission","message":"Run shell?"}"#;
        let e = parse_with_kind("gemini", EmitKind::Notification, gemini, None, 1).unwrap();
        assert_eq!(e.kind, EventKind::ApprovalRequired);
        assert_eq!(e.title, "gemini needs approval");
        assert_eq!(e.summary, "Run shell?");
        let copilot =
            r#"{"hook_event_name":"notification","notification_type":"idle","message":"Waiting"}"#;
        let q = parse_with_kind("copilot", EmitKind::Notification, copilot, None, 1).unwrap();
        assert_eq!(q.kind, EventKind::Question);
    }
}
