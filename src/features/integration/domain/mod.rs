//! Pure edits of a Claude Code `settings.json` value.
//!
//! Adds one `{"type":"command","command":"<exe> emit --source claude"}` hook
//! under `hooks.Notification` and `hooks.Stop`. Unrelated keys and hooks are
//! preserved; both operations are idempotent.

use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

pub const HOOK_EVENTS: &[&str] = &["Notification", "Stop"];
const MARKER: &str = "emit --source claude";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrationError {
    #[error("settings file is not a JSON object")]
    NotAnObject,
    #[error("settings file has an unexpected `hooks` shape; edit it manually")]
    BadHooks,
    #[error("settings file could not be read or written")]
    Io,
}

impl ErrorCode for IntegrationError {
    fn code(&self) -> &'static str {
        match self {
            IntegrationError::NotAnObject | IntegrationError::BadHooks => "settings_invalid",
            IntegrationError::Io => "io_error",
        }
    }
}

/// Single-quotes `s` for a POSIX shell (Claude Code runs hook commands via a shell).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn hook_command(exe: &str) -> String {
    format!("{} {MARKER}", shell_quote(exe))
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("openmosh-hook") && c.ends_with(MARKER))
}

fn hooks_obj(settings: &mut Value) -> Result<&mut Map<String, Value>, IntegrationError> {
    let obj = settings
        .as_object_mut()
        .ok_or(IntegrationError::NotAnObject)?;
    obj.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or(IntegrationError::BadHooks)
}

/// Adds the hooks. Returns whether `settings` changed.
pub fn install(settings: &mut Value, command: &str) -> Result<bool, IntegrationError> {
    let hooks = hooks_obj(settings)?;
    let mut changed = false;
    for ev in HOOK_EVENTS {
        let groups = hooks
            .entry(*ev)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or(IntegrationError::BadHooks)?;
        let present = groups.iter().any(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.iter().any(is_ours))
        });
        if !present {
            groups.push(json!({"hooks": [{"type": "command", "command": command}]}));
            changed = true;
        }
    }
    Ok(changed)
}

/// Removes only our hooks, dropping groups/events/`hooks` left empty by that.
pub fn uninstall(settings: &mut Value) -> Result<bool, IntegrationError> {
    let obj = settings
        .as_object_mut()
        .ok_or(IntegrationError::NotAnObject)?;
    let Some(hooks) = obj.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks.as_object_mut().ok_or(IntegrationError::BadHooks)?;
    let mut changed = false;
    for ev in HOOK_EVENTS {
        let Some(groups) = hooks.get_mut(*ev).and_then(Value::as_array_mut) else {
            continue;
        };
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h));
                if hs.len() != before {
                    changed = true;
                }
            }
        }
        groups.retain(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|hs| !hs.is_empty())
        });
        if groups.is_empty() {
            hooks.remove(*ev);
        }
    }
    if changed && hooks.is_empty() {
        obj.remove("hooks");
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "'/home/u/.local/bin/openmosh-hook' emit --source claude";

    #[test]
    fn install_preserves_unrelated_and_is_idempotent() {
        let mut s = json!({
            "model": "opus",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}], "PreToolUse": []}
        });
        assert!(install(&mut s, CMD).unwrap());
        assert!(!install(&mut s, CMD).unwrap(), "second install is a no-op");
        assert_eq!(s["model"], "opus");
        assert_eq!(s["hooks"]["PreToolUse"], json!([]));
        assert_eq!(
            s["hooks"]["Stop"][0]["hooks"][0]["command"],
            "notify-send done"
        );
        assert_eq!(s["hooks"]["Stop"][1]["hooks"][0]["command"], CMD);
        assert_eq!(
            s["hooks"]["Notification"][0]["hooks"][0],
            json!({"type": "command", "command": CMD})
        );
    }

    #[test]
    fn uninstall_removes_only_ours() {
        let original = json!({"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]}});
        let mut s = original.clone();
        install(&mut s, CMD).unwrap();
        assert!(uninstall(&mut s).unwrap());
        assert_eq!(s, original);
        assert!(!uninstall(&mut s).unwrap());

        let mut empty = json!({});
        install(&mut empty, CMD).unwrap();
        uninstall(&mut empty).unwrap();
        assert_eq!(empty, json!({}));
    }

    #[test]
    fn refuses_unexpected_shapes() {
        assert_eq!(
            install(&mut json!([]), CMD),
            Err(IntegrationError::NotAnObject)
        );
        assert_eq!(
            install(&mut json!({"hooks": []}), CMD),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            install(&mut json!({"hooks": {"Stop": {}}}), CMD),
            Err(IntegrationError::BadHooks)
        );
    }

    #[test]
    fn command_is_shell_quoted() {
        assert_eq!(
            hook_command("/opt/it's/openmosh-hook"),
            r"'/opt/it'\''s/openmosh-hook' emit --source claude"
        );
    }
}
