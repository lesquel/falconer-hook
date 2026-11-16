//! `setup` and `uninstall` against a temporary HOME/XDG tree and fake
//! service managers. Nothing here touches the real machine.

mod common;

use common::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Every file and directory under `root` with its bytes and mode.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let m = fs::symlink_metadata(&p).unwrap();
        let mode = m.permissions().mode() & 0o7777;
        if m.is_dir() {
            out.insert(p.clone(), (Vec::new(), mode));
            for e in fs::read_dir(&p).unwrap() {
                stack.push(e.unwrap().path());
            }
        } else {
            out.insert(p.clone(), (fs::read(&p).unwrap(), mode));
        }
    }
    out
}

const USER_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH me@laptop";
const CLAUDE_ORIGINAL: &str = "{\n  \"model\": \"opus\"\n}\n";

/// A machine where falconer-hook was set up: service, Claude hooks, one
/// paired phone key next to the user's own key, state and config.
fn installed() -> Env {
    let e = Env::isolated();
    let claude = e.home().join(".claude/settings.json");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::write(&claude, CLAUDE_ORIGINAL).unwrap();
    let (code, v) = e.json(&["setup", "--yes", "--json"], None, &[]);
    let d = ok_data(code, &v).clone();
    assert_eq!(d["next"], "falconer-hook pair");
    let req = r#"{"name":"Pixel 8","ppk":"B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw","endpoint":"https://ntfy.sh/upTest?up=1"}"#;
    let (code, v) = e.json(&["pair-ssh", "--json"], Some(req), &[]);
    ok_data(code, &v);
    assert!(e.state().join("devices.toml").exists());
    let ssh = e.home().join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let ak = ssh.join("authorized_keys");
    fs::write(
        &ak,
        format!(
            "{USER_KEY}\nfrom=\"100.64.0.0/10\",no-pty {} openmosh:AAAAAAAAAAAAAAAAAAAA01\n",
            &USER_KEY[..USER_KEY.len() - 10]
        ),
    )
    .unwrap();
    fs::set_permissions(&ak, fs::Permissions::from_mode(0o600)).unwrap();
    e
}

#[test]
fn setup_yes_installs_service_and_detected_integrations() {
    let e = Env::isolated();
    fs::create_dir_all(e.home().join(".claude")).unwrap();
    fs::create_dir_all(e.home().join(".codex")).unwrap();
    let cfg_before = fs::read(e.root.join("config/falconer/config.toml")).unwrap();
    let (code, v) = e.json(&["setup", "--yes", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["config"]["created"], false);
    assert_eq!(
        fs::read(e.root.join("config/falconer/config.toml")).unwrap(),
        cfg_before,
        "existing config kept"
    );
    assert_eq!(d["service"]["installed"], true);
    assert!(e
        .root
        .join("config/systemd/user/falconer-hook.service")
        .exists());
    let actions: BTreeMap<String, String> = d["integrations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["agent"].as_str().unwrap().into(),
                a["action"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(actions.get("claude").map(String::as_str), Some("installed"));
    assert_eq!(actions.get("codex").map(String::as_str), Some("installed"));
    assert!(
        !actions.contains_key("kiro"),
        "undetected agents are not touched"
    );
    assert!(!e.home().join(".kiro").exists());
    assert!(d["doctor"]["checks"].is_array());
    let calls = e.calls();
    assert!(calls
        .iter()
        .any(|c| c[1..] == ["--user", "enable", "--now", "falconer-hook.service"]));

    let (code, v) = e.json(&["setup", "--yes", "--json"], None, &[]);
    let again = &ok_data(code, &v)["integrations"];
    assert!(again
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["action"] == "already_installed"));
}

#[test]
fn setup_without_yes_offers_but_changes_no_agent_file() {
    let e = Env::isolated();
    fs::create_dir_all(e.home().join(".codex")).unwrap();
    fs::remove_file(e.root.join("config/falconer/config.toml")).unwrap();
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["config"]["created"], true);
    assert_eq!(d["service"]["skipped"], true);
    let cfg = e.root.join("config/falconer/config.toml");
    assert_eq!(mode(&cfg), 0o600);
    assert!(fs::read_to_string(&cfg)
        .unwrap()
        .contains("retention_days = 7"));
    assert_eq!(d["integrations"][0]["agent"], "codex");
    assert_eq!(d["integrations"][0]["action"], "skipped");
    assert!(!e.home().join(".codex/hooks.json").exists());
    assert!(!e.root.join("config/systemd").exists());
}

#[test]
fn uninstall_dry_run_and_unconfirmed_change_nothing() {
    let e = installed();
    let before = snapshot(&e.root);
    let (code, v) = e.json(&["uninstall", "--dry-run", "--all", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let steps: Vec<_> = d["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["step"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        steps,
        [
            "service",
            "integration",
            "authorized_keys",
            "remove_dir",
            "remove_dir",
            "remove_binary"
        ]
    );
    assert_eq!(d["steps"][2]["lines"], 1);
    let (code, v) = e.json(&["uninstall", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "confirmation_required");
    let out = e.run(&["uninstall"], None, &[]);
    assert_eq!(out.status.code(), Some(1), "non-interactive without --yes");
    assert_eq!(snapshot(&e.root), before, "nothing changed");
}

#[test]
fn uninstall_yes_removes_only_what_falconer_created() {
    let e = installed();
    let calls_before = e.calls().len();
    let (code, v) = e.json(&["uninstall", "--yes", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["complete"], true, "{d}");
    assert!(!e
        .root
        .join("config/systemd/user/falconer-hook.service")
        .exists());
    assert!(e.calls()[calls_before..]
        .iter()
        .any(|c| c[1..] == ["--user", "disable", "--now", "falconer-hook.service"]));
    let claude: Value =
        serde_json::from_str(&fs::read_to_string(e.home().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        claude,
        serde_json::from_str::<Value>(CLAUDE_ORIGINAL).unwrap()
    );
    assert_eq!(
        fs::read_to_string(e.home().join(".ssh/authorized_keys")).unwrap(),
        format!("{USER_KEY}\n"),
        "only the falconer: line is removed"
    );
    assert!(!e.root.join("config/falconer").exists());
    assert!(!e.state().exists());
    assert!(e.root.join("config").exists(), "parent dirs are kept");
    assert!(Path::new(BIN).exists(), "the binary needs --all");

    let (code, v) = e.json(&["uninstall", "--yes", "--json"], None, &[]);
    assert_eq!(ok_data(code, &v)["steps"], serde_json::json!([]));
}

#[test]
fn uninstall_all_deletes_the_running_binary() {
    let e = Env::isolated();
    let bin = e.root.join("bin/falconer-hook");
    fs::copy(BIN, &bin).unwrap();
    // Run the copy, never BIN itself.
    let out = std::process::Command::new(&bin)
        .args(["uninstall", "--all", "--yes", "--json"])
        .env_clear()
        .env("HOME", e.home())
        .env("XDG_CONFIG_HOME", e.root.join("config"))
        .env("XDG_STATE_HOME", e.root.join("state"))
        .env("PATH", &e.path)
        .env("FAKE_LOG", e.root.join("calls.log"))
        .env("FAKE_DIR", e.root.join("fake"))
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["complete"], true, "{v}");
    assert!(!bin.exists(), "the copy deleted itself");
    assert!(Path::new(BIN).exists());
}

#[test]
fn uninstall_removes_tagged_keys_even_without_state() {
    let e = Env::isolated();
    let ssh = e.home().join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let ak = ssh.join("authorized_keys");
    fs::write(
        &ak,
        format!("{USER_KEY}\nssh-ed25519 AAAA openmosh:AAAAAAAAAAAAAAAAAAAA09\n"),
    )
    .unwrap();
    let (code, v) = e.json(&["uninstall", "--yes", "--json"], None, &[]);
    assert_eq!(ok_data(code, &v)["complete"], true, "{v}");
    assert_eq!(fs::read_to_string(&ak).unwrap(), format!("{USER_KEY}\n"));
    assert!(!e.state().exists(), "no state dir left behind");
}
