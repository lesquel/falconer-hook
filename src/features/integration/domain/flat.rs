//! `{"version": 1, "hooks": {"<event>": [{"command": "…"}]}}`, Cursor's
//! `hooks.json`.

use serde_json::{json, Map, Value};

use super::nested::Hook;
use super::{is_our_command, IntegrationError};

fn is_ours(entry: &Value, agent: &str) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| is_our_command(c, agent))
}

fn hooks_obj(settings: &mut Value) -> Result<&mut Map<String, Value>, IntegrationError> {
    let obj = settings
        .as_object_mut()
        .ok_or(IntegrationError::NotAnObject)?;
    match obj.get("version") {
        None => {
            obj.insert("version".into(), json!(1));
        }
        Some(v) if v.is_u64() => {}
        Some(_) => return Err(IntegrationError::BadHooks),
    }
    obj.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or(IntegrationError::BadHooks)
}

pub fn install(
    settings: &mut Value,
    agent: &str,
    wanted: &[Hook],
) -> Result<bool, IntegrationError> {
    let had_version = settings.get("version").is_some();
    let hooks = hooks_obj(settings)?;
    let mut changed = !had_version;
    for w in wanted {
        let entries = hooks
            .entry(w.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or(IntegrationError::BadHooks)?;
        let mut present = false;
        let before = entries.len();
        // Keep the first of ours, repointed; drop any later copy.
        entries.retain_mut(|e| {
            if !is_ours(e, agent) {
                return true;
            }
            if present {
                return false;
            }
            present = true;
            if e.get("command").and_then(Value::as_str) != Some(w.command.as_str()) {
                e["command"] = Value::String(w.command.clone());
                changed = true;
            }
            true
        });
        changed |= entries.len() != before;
        if !present {
            entries.push(json!({"command": w.command}));
            changed = true;
        }
    }
    Ok(changed)
}

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
    for (ev, entries) in hooks.iter_mut() {
        let Some(entries) = entries.as_array_mut() else {
            continue;
        };
        let before = entries.len();
        entries.retain(|e| !is_ours(e, agent));
        if entries.len() != before {
            changed = true;
            if entries.is_empty() {
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
        .any(|e| is_ours(e, agent)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "'/b/falconer-hook' emit --source cursor --kind done";

    fn stop() -> Vec<Hook<'static>> {
        vec![Hook {
            event: "stop",
            command: CMD.into(),
        }]
    }

    #[test]
    fn install_replaces_openmosh_entries_and_drops_copies() {
        let old = "'/b/openmosh-hook' emit --source cursor --kind done";
        let mut s = json!({"version": 1, "hooks": {"stop": [{"command": old}, {"command": "say done"}, {"command": CMD}]}});
        assert!(install(&mut s, "cursor", &stop()).unwrap());
        assert_eq!(
            s["hooks"]["stop"],
            json!([{"command": CMD}, {"command": "say done"}])
        );
    }

    #[test]
    fn install_is_idempotent_and_uninstall_restores() {
        let original = json!({"version": 1, "hooks": {"afterFileEdit": [{"command": "./hooks/format.sh"}], "stop": [{"command": "say done"}]}});
        let mut s = original.clone();
        assert!(install(&mut s, "cursor", &stop()).unwrap());
        assert!(!install(&mut s, "cursor", &stop()).unwrap());
        assert_eq!(s["hooks"]["stop"][1], json!({"command": CMD}));
        assert!(installed(&s, "cursor").unwrap());
        assert!(uninstall(&mut s, "cursor").unwrap());
        assert_eq!(s, original);
        assert!(!installed(&s, "cursor").unwrap());
    }

    #[test]
    fn new_file_gets_a_version() {
        let mut s = json!({});
        assert!(install(&mut s, "cursor", &stop()).unwrap());
        assert_eq!(
            s,
            json!({"version": 1, "hooks": {"stop": [{"command": CMD}]}})
        );
        uninstall(&mut s, "cursor").unwrap();
        assert_eq!(s, json!({"version": 1}));
    }

    #[test]
    fn refuses_unexpected_shapes() {
        assert_eq!(
            install(&mut json!({"version": "1"}), "cursor", &stop()),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            install(&mut json!({"hooks": {"stop": {}}}), "cursor", &stop()),
            Err(IntegrationError::BadHooks)
        );
        assert_eq!(
            install(&mut json!(1), "cursor", &stop()),
            Err(IntegrationError::NotAnObject)
        );
    }
}
