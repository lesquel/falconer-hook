//! Upgrading a machine that still runs OpenMosh (`openmosh-hook`): its
//! config/state directories, user service and agent hooks move to the
//! Falconer names. Everything runs against a temporary HOME/XDG tree.

mod common;

use common::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

const OLD_CLAUDE_STOP: &str = "'/home/u/.local/bin/openmosh-hook' emit --source claude";

/// Turns the fresh `Env` into an OpenMosh-era machine: the config lives in
/// `config/openmosh`, the state (devices, host key, inbox) in `state/openmosh`.
fn legacy_env() -> Env {
    let e = Env::isolated();
    fs::rename(
        e.root.join("config/falconer"),
        e.root.join("config/openmosh"),
    )
    .unwrap();
    let state = e.root.join("state/openmosh");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("host.key"), b"old-host-key").unwrap();
    fs::write(state.join("devices.toml"), b"").unwrap();
    e
}

fn checks(d: &Value) -> Vec<(String, String, String)> {
    d["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_string(),
                c["status"].as_str().unwrap().to_string(),
                c["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

#[test]
fn legacy_directories_move_when_the_new_ones_are_missing() {
    let e = legacy_env();
    let cfg_before = fs::read(e.root.join("config/openmosh/config.toml")).unwrap();
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(
        fs::read(e.root.join("config/falconer/config.toml")).unwrap(),
        cfg_before,
        "config kept byte for byte"
    );
    assert_eq!(
        fs::read(e.root.join("state/falconer/host.key")).unwrap(),
        b"old-host-key",
        "pairing key kept"
    );
    assert!(!e.root.join("config/openmosh").exists());
    assert!(!e.root.join("state/openmosh").exists());
    assert!(
        checks(d).iter().all(|(n, _, _)| n != "legacy"),
        "nothing left over: {d}"
    );
}

#[test]
fn legacy_directories_are_untouched_when_the_new_ones_exist() {
    let e = Env::isolated();
    let old_cfg = e.root.join("config/openmosh/config.toml");
    write(&old_cfg, "# old\n");
    let new_before = fs::read(e.root.join("config/falconer/config.toml")).unwrap();
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(fs::read_to_string(&old_cfg).unwrap(), "# old\n");
    assert_eq!(
        fs::read(e.root.join("config/falconer/config.toml")).unwrap(),
        new_before
    );
    let legacy: Vec<_> = checks(d)
        .into_iter()
        .filter(|(n, _, _)| n == "legacy")
        .collect();
    assert_eq!(legacy.len(), 1, "{d}");
    assert_eq!(legacy[0].1, "warn");
    assert!(
        legacy[0]
            .2
            .contains(&e.root.join("config/openmosh").display().to_string()),
        "{legacy:?}"
    );
}

#[test]
fn doctor_reports_a_legacy_service_and_legacy_agent_hooks() {
    let e = Env::isolated();
    write(
        &e.root.join("config/systemd/user/openmosh-hook.service"),
        "[Service]\n",
    );
    write(
        &e.home().join(".claude/settings.json"),
        &format!(
            r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{OLD_CLAUDE_STOP}"}}]}}]}}}}"#
        ),
    );
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let legacy = checks(d)
        .into_iter()
        .find(|(n, _, _)| n == "legacy")
        .expect("legacy check");
    assert_eq!(legacy.1, "warn");
    assert!(legacy.2.contains("openmosh-hook.service"), "{legacy:?}");
    assert!(legacy.2.contains("claude"), "{legacy:?}");
    assert!(legacy.2.contains("falconer-hook setup"), "{legacy:?}");
}

#[test]
fn setup_replaces_the_legacy_service() {
    let e = legacy_env();
    let old_unit = e.root.join("config/systemd/user/openmosh-hook.service");
    write(&old_unit, "[Service]\nExecStart=/x/openmosh-hook serve\n");
    let (code, v) = e.json(&["setup", "--yes", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["service"]["installed"], true, "{d}");
    assert!(!old_unit.exists(), "old unit removed");
    assert!(e
        .root
        .join("config/systemd/user/falconer-hook.service")
        .exists());
    let calls: Vec<Vec<String>> = e
        .calls()
        .into_iter()
        .filter(|c| c[0] == "systemctl")
        .map(|c| c[1..].to_vec())
        .collect();
    let pos = |want: &[&str]| {
        calls
            .iter()
            .position(|c| c == want)
            .unwrap_or_else(|| panic!("{want:?} not run: {calls:?}"))
    };
    let stop_old = pos(&["--user", "disable", "--now", "openmosh-hook.service"]);
    let start_new = pos(&["--user", "enable", "--now", "falconer-hook.service"]);
    assert!(stop_old < start_new, "{calls:?}");
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    assert!(
        checks(ok_data(code, &v))
            .iter()
            .all(|(n, _, _)| n != "legacy"),
        "{v}"
    );
}

#[test]
fn setup_rewrites_legacy_agent_hooks_without_duplicates_or_foreign_changes() {
    let e = legacy_env();
    let claude = e.home().join(".claude/settings.json");
    write(
        &claude,
        &format!(
            r#"{{
  "model": "opus",
  "hooks": {{
    "Notification": [{{"hooks": [{{"type": "command", "command": "{OLD_CLAUDE_STOP}"}}]}}],
    "Stop": [
      {{"hooks": [{{"type": "command", "command": "echo mine"}}]}},
      {{"hooks": [{{"type": "command", "command": "{OLD_CLAUDE_STOP}"}}]}},
      {{"hooks": [{{"type": "command", "command": "'/b/falconer-hook' emit --source claude"}}]}}
    ]
  }}
}}"#
        ),
    );
    let old_plugin = e.root.join("config/opencode/plugins/openmosh.js");
    write(
        &old_plugin,
        "// openmosh-hook integration: managed by `openmosh-hook integration install opencode`.\nconst EXE = \"/x/openmosh-hook\";\n",
    );
    let foreign_plugin = e.root.join("config/opencode/plugins/other.js");
    write(&foreign_plugin, "// someone else's plugin\n");
    let aider = e.home().join(".aider.conf.yml");
    write(
        &aider,
        "model: gpt\n# >>> openmosh-hook (managed: `openmosh-hook integration uninstall aider` removes it)\nnotifications: true\nnotifications_command: '''/x/openmosh-hook'' emit --source aider --kind done < /dev/null'\n# <<< openmosh-hook\n",
    );

    let (code, v) = e.json(&["setup", "--yes", "--json"], None, &[]);
    ok_data(code, &v);

    let text = fs::read_to_string(&claude).unwrap();
    assert!(!text.contains("openmosh-hook"), "{text}");
    let s: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(s["model"], "opus");
    for ev in ["Notification", "Stop"] {
        let ours = s["hooks"][ev]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["hooks"].as_array().unwrap().iter())
            .filter(|h| h["command"].as_str().unwrap().contains("falconer-hook"))
            .count();
        assert_eq!(ours, 1, "{ev}: {text}");
    }
    assert!(text.contains("echo mine"), "foreign hook kept: {text}");

    assert!(!old_plugin.exists(), "old plugin removed");
    let new_plugin = e.root.join("config/opencode/plugins/falconer.js");
    assert!(fs::read_to_string(&new_plugin)
        .unwrap()
        .contains("falconer-hook"));
    assert_eq!(
        fs::read_to_string(&foreign_plugin).unwrap(),
        "// someone else's plugin\n"
    );

    let y = fs::read_to_string(&aider).unwrap();
    assert!(!y.contains("openmosh-hook"), "{y}");
    assert!(y.starts_with("model: gpt\n# >>> falconer-hook"), "{y}");
    assert_eq!(y.matches("notifications:").count(), 1, "{y}");
}
