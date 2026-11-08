//! Agent integrations through the CLI, against a temporary HOME only.

mod common;

use common::*;
use serde_json::Value;
use std::fs;

fn agent<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent"] == id)
        .unwrap_or_else(|| panic!("{id} missing in {list}"))
}

#[test]
fn list_shows_every_agent_with_detection_and_install_state() {
    let e = Env::isolated();
    fs::create_dir_all(e.home().join(".codex")).unwrap();
    let (code, v) = e.json(&["integration", "list", "--json"], None, &[]);
    let d = ok_data(code, &v).clone();
    for id in [
        "claude", "codex", "gemini", "cursor", "copilot", "kiro", "opencode", "aider",
    ] {
        let a = agent(&d, id);
        assert_eq!(a["installed"], false, "{id}");
        assert!(a["doc"].as_str().unwrap().starts_with("https://"));
    }
    assert_eq!(agent(&d, "codex")["detected"], true);
    assert_eq!(
        agent(&d, "codex")["settings"],
        e.home().join(".codex/hooks.json").display().to_string()
    );
    assert_eq!(
        agent(&d, "opencode")["settings"],
        e.root
            .join("config/opencode/plugins/openmosh.js")
            .display()
            .to_string(),
        "OpenCode follows XDG_CONFIG_HOME"
    );

    let (code, v) = e.json(&["integration", "install", "codex", "--json"], None, &[]);
    let r = ok_data(code, &v);
    assert_eq!(r["changed"], true);
    assert!(r["note"].as_str().unwrap().contains("trust"));
    let (code, v) = e.json(&["integration", "list", "--json"], None, &[]);
    assert_eq!(agent(ok_data(code, &v), "codex")["installed"], true);
}

#[test]
fn every_agent_installs_idempotently_and_uninstalls_cleanly() {
    let e = Env::isolated();
    for id in [
        "codex", "gemini", "cursor", "copilot", "kiro", "opencode", "aider",
    ] {
        let (code, v) = e.json(&["integration", "install", id, "--json"], None, &[]);
        let path = ok_data(code, &v)["settings"].as_str().unwrap().to_string();
        let text = fs::read_to_string(&path).unwrap();
        let marker = if id == "opencode" {
            r#""emit", "--source", "opencode""#.to_string()
        } else {
            format!(" emit --source {id}")
        };
        assert!(text.contains(&marker), "{id}: {text}");
        assert!(text.contains(BIN) || text.contains(&BIN.replace('"', "\\\"")));
        assert_eq!(mode(std::path::Path::new(&path)), 0o600, "{id}");
        let (code, v) = e.json(&["integration", "install", id, "--json"], None, &[]);
        assert_eq!(ok_data(code, &v)["changed"], false, "{id} idempotent");
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "{id} unchanged");
        let (code, v) = e.json(&["integration", "uninstall", id, "--json"], None, &[]);
        assert_eq!(ok_data(code, &v)["changed"], true, "{id}");
        let left = fs::read_to_string(&path).unwrap_or_default();
        assert!(!left.contains("openmosh-hook"), "{id}: {left}");
    }
}

#[test]
fn malformed_and_foreign_files_are_refused_untouched() {
    let e = Env::isolated();
    let gemini = e.home().join(".gemini/settings.json");
    fs::create_dir_all(gemini.parent().unwrap()).unwrap();
    fs::write(&gemini, "{ // comment\n \"a\": 1 }").unwrap();
    let (code, v) = e.json(&["integration", "install", "gemini", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "settings_invalid");
    assert_eq!(
        fs::read_to_string(&gemini).unwrap(),
        "{ // comment\n \"a\": 1 }"
    );
    let kiro = e.home().join(".kiro/hooks/openmosh.json");
    fs::create_dir_all(kiro.parent().unwrap()).unwrap();
    fs::write(&kiro, "{\"version\":\"v1\",\"hooks\":[]}").unwrap();
    let (code, v) = e.json(&["integration", "install", "kiro", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "settings_conflict");
    let (code, v) = e.json(&["integration", "uninstall", "kiro", "--json"], None, &[]);
    assert_eq!(ok_data(code, &v)["changed"], false);
    assert!(kiro.exists(), "a file we did not write is never deleted");
}

#[test]
fn emit_with_kind_records_agent_payloads() {
    let e = Env::isolated();
    let (code, v) = e.json(
        &["emit", "--source", "codex", "--kind", "done", "--json"],
        Some(r#"{"hook_event_name":"Stop","last_assistant_message":"Refactor finished"}"#),
        &[("TMUX_PANE", "%2")],
    );
    assert_eq!(ok_data(code, &v)["recorded"], true);
    let (code, v) = e.json(
        &["emit", "--source", "aider", "--kind", "done", "--json"],
        None,
        &[],
    );
    assert_eq!(ok_data(code, &v)["recorded"], true, "empty payload is fine");
    let (code, v) = e.json(
        &[
            "emit",
            "--source",
            "gemini",
            "--kind",
            "notification",
            "--json",
        ],
        Some(r#"{"notification_type":"ToolPermission","message":"Run ls?"}"#),
        &[],
    );
    assert_eq!(ok_data(code, &v)["recorded"], true);
    let (code, v) = e.json(&["inbox", "list", "--json"], None, &[]);
    let events = ok_data(code, &v)["events"].as_array().unwrap().clone();
    let by = |a: &str| events.iter().find(|ev| ev["agent"] == a).unwrap().clone();
    assert_eq!(by("codex")["kind"], "done");
    assert_eq!(by("codex")["summary"], "Refactor finished");
    assert_eq!(by("aider")["title"], "aider finished");
    assert_eq!(by("gemini")["kind"], "approval_required");

    let out = e.run(
        &["emit", "--source", "codex", "--kind", "bogus"],
        Some("{}"),
        &[],
    );
    assert_eq!(out.status.code(), Some(2), "clap rejects unknown kinds");
}
