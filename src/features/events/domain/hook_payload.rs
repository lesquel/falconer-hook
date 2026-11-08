//! Maps an agent hook payload (JSON on stdin of `emit`) to a `NewEvent`.
//!
//! Supported shapes:
//! - Claude Code hooks: `hook_event_name` = `Notification` | `PermissionRequest` | `Stop`.
//! - Generic: `{"kind":"approval_required|question|done","title":"…","summary":"…"}`.
//!
//! Only `message`/`title`/`summary` are kept; transcript paths, session ids
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
    let clean = |s: String| s.chars().filter(|c| !c.is_control()).collect::<String>();
    Ok(NewEvent {
        source: source.to_string(),
        agent,
        kind,
        title: clean(title).chars().take(120).collect(),
        summary: clean(summary).chars().take(200).collect(),
        pane,
        created_at: now,
    })
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
}
