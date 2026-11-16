//! `hooks.<Event>: [{matcher?, hooks: [{type: "command", command}]}]`, the
//! shape shared by Claude Code `settings.json`, Codex `hooks.json` and
//! Gemini CLI `settings.json`.

use serde_json::{json, Map, Value};

use super::{is_our_command, IntegrationError};

/// One hook to add: the agent event and the full command for it.
pub struct Hook<'a> {
    pub event: &'a str,
    pub command: String,
}

fn is_ours(hook: &Value, agent: &str) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| is_our_command(c, agent))
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

/// Adds the hooks, or repoints ours when the command changed (e.g. the
/// binary moved). Returns whether `settings` changed.
pub fn install(
    settings: &mut Value,
    agent: &str,
    wanted: &[Hook],
    matcher: Option<&str>,
    name: Option<&str>,
) -> Result<bool, IntegrationError> {
    let hooks = hooks_obj(settings)?;
    let mut changed = false;
    for w in wanted {
        let groups = hooks
            .entry(w.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or(IntegrationError::BadHooks)?;
        let mut present = false;
        let mut emptied = false;
        for g in groups.iter_mut() {
            let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = hs.len();
            // Keep the first of ours, repointed; drop any later copy (an
            // upgrade can leave the old and the new command side by side).
            hs.retain_mut(|h| {
                if !is_ours(h, agent) {
                    return true;
                }
                if present {
                    return false;
                }
                present = true;
                if h.get("command").and_then(Value::as_str) != Some(w.command.as_str()) {
                    h["command"] = Value::String(w.command.clone());
                    changed = true;
                }
                if let Some(n) = name {
                    if h.get("name").is_some_and(|v| v.as_str() != Some(n)) {
                        h["name"] = json!(n);
                        changed = true;
                    }
                }
                true
            });
            if hs.len() != before {
                changed = true;
                emptied |= hs.is_empty();
            }
        }
        if emptied {
            groups.retain(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|hs| !hs.is_empty())
            });
        }
        if !present {
            let mut hook = Map::new();
            if let Some(n) = name {
                hook.insert("name".into(), json!(n));
            }
            hook.insert("type".into(), json!("command"));
            hook.insert("command".into(), json!(w.command));
            let mut group = Map::new();
            if let Some(m) = matcher {
                group.insert("matcher".into(), json!(m));
            }
            group.insert("hooks".into(), json!([hook]));
            groups.push(Value::Object(group));
            changed = true;
        }
    }
    Ok(changed)
}

/// Removes only our hooks (under any event), dropping groups/events/`hooks`
/// left empty by that.
pub fn uninstall(settings: &mut Value, agent: &str) -> Result<bool, IntegrationError> {
    let obj = settings
        .as_object_mut()
        .ok_or(IntegrationError::NotAnObject)?;
    let Some(hooks) = obj.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks.as_object_mut().ok_or(IntegrationError::BadHooks)?;
    let mut changed = false;
    let mut emptied = Vec::new();
    for (ev, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let mut touched = false;
        for g in groups.iter_mut() {
            if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h, agent));
                touched |= hs.len() != before;
            }
        }
        if touched {
            changed = true;
            groups.retain(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|hs| !hs.is_empty())
            });
            if groups.is_empty() {
                emptied.push(ev.clone());
            }
        }
    }
    for ev in emptied {
        hooks.remove(&ev);
    }
    if changed && hooks.is_empty() {
        obj.remove("hooks");
    }
    Ok(changed)
}

/// Whether any of our hooks is present.
pub fn installed(settings: &Value, agent: &str) -> Result<bool, IntegrationError> {
    let obj = settings.as_object().ok_or(IntegrationError::NotAnObject)?;
    let Some(hooks) = obj.get("hooks") else {
        return Ok(false);
    };
    let hooks = hooks.as_object().ok_or(IntegrationError::BadHooks)?;
    Ok(hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|g| g.get("hooks").and_then(Value::as_array))
        .flatten()
        .any(|h| is_ours(h, agent)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "'/home/u/.local/bin/falconer-hook' emit --source claude";

    fn claude(cmd: &str) -> Vec<Hook<'static>> {
        ["Notification", "Stop"]
            .iter()
            .map(|e| Hook {
                event: e,
                command: cmd.into(),
            })
            .collect()
    }

    #[test]
    fn install_preserves_unrelated_and_is_idempotent() {
        let mut s = json!({
            "model": "opus",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}], "PreToolUse": []}
        });
        assert!(install(&mut s, "claude", &claude(CMD), None, None).unwrap());
        assert!(
            !install(&mut s, "claude", &claude(CMD), None, None).unwrap(),
            "second install is a no-op"
        );
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
        assert!(installed(&s, "claude").unwrap());
        assert!(!installed(&s, "codex").unwrap());
    }

    #[test]
    fn install_repoints_a_stale_executable_path() {
        let old = "'/old/place/falconer-hook' emit --source claude";
        let mut s = json!({"hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}, {"type": "command", "command": old}]}],
            "Notification": [{"hooks": [{"type": "command", "command": CMD}]}]
        }});
        assert!(install(&mut s, "claude", &claude(CMD), None, None).unwrap());
        assert_eq!(
            s["hooks"]["Stop"][0]["hooks"][0]["command"],
            "notify-send done"
        );
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][1]["command"], CMD);
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(
            !install(&mut s, "claude", &claude(CMD), None, None).unwrap(),
            "idempotent once repointed"
        );
    }

    #[test]
    fn uninstall_removes_only_ours() {
        let original = json!({"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}], "Other": "x"}});
        let mut s = original.clone();
        install(&mut s, "claude", &claude(CMD), None, None).unwrap();
        assert!(uninstall(&mut s, "claude").unwrap());
        assert_eq!(s, original);
        assert!(!uninstall(&mut s, "claude").unwrap());

        let mut empty = json!({});
        install(&mut empty, "claude", &claude(CMD), None, None).unwrap();
        uninstall(&mut empty, "claude").unwrap();
        assert_eq!(empty, json!({}));
    }

    #[test]
    fn other_agents_hooks_survive_uninstall() {
        let mut s = json!({});
        install(&mut s, "claude", &claude(CMD), None, None).unwrap();
        let codex = "'/b/falconer-hook' emit --source codex --kind done";
        install(
            &mut s,
            "codex",
            &[Hook {
                event: "Stop",
                command: codex.into(),
            }],
            None,
            None,
        )
        .unwrap();
        uninstall(&mut s, "claude").unwrap();
        assert_eq!(
            s,
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": codex}]}]}})
        );
    }

    #[test]
    fn matcher_and_name_are_written() {
        let mut s = json!({"general": {"vimMode": true}});
        let cmd = "'/b/falconer-hook' emit --source gemini --kind done";
        install(
            &mut s,
            "gemini",
            &[Hook {
                event: "AfterAgent",
                command: cmd.into(),
            }],
            Some("*"),
            Some("falconer"),
        )
        .unwrap();
        assert_eq!(
            s["hooks"]["AfterAgent"][0],
            json!({"matcher": "*", "hooks": [{"name": "falconer", "type": "command", "command": cmd}]})
        );
    }

    #[test]
    fn install_renames_an_openmosh_hook_in_place() {
        let cmd = "'/b/falconer-hook' emit --source gemini --kind done";
        let old = "'/b/openmosh-hook' emit --source gemini --kind done";
        let mut s = json!({"hooks": {"AfterAgent": [{"matcher": "*", "hooks": [{"name": "openmosh", "type": "command", "command": old}]}]}});
        let h = [Hook {
            event: "AfterAgent",
            command: cmd.into(),
        }];
        assert!(install(&mut s, "gemini", &h, Some("*"), Some("falconer")).unwrap());
        assert_eq!(
            s["hooks"]["AfterAgent"],
            json!([{"matcher": "*", "hooks": [{"name": "falconer", "type": "command", "command": cmd}]}])
        );
        assert!(!install(&mut s, "gemini", &h, Some("*"), Some("falconer")).unwrap());
    }

    #[test]
    fn refuses_unexpected_shapes() {
        let h = claude(CMD);
        assert_eq!(
            install(&mut json!([]), "claude", &h, None, None),
            Err(IntegrationError::NotAnObject)
        );
        assert_eq!(
            install(&mut json!({"hooks": []}), "claude", &h, None, None),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            install(
                &mut json!({"hooks": {"Stop": {}}}),
                "claude",
                &h,
                None,
                None
            ),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            installed(&json!({"hooks": 1}), "claude"),
            Err(IntegrationError::BadHooks)
        );
    }
}
