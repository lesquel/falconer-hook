//! `doctor` fix ids (`fix: {id, description}`) and `doctor --fix [<id>]`.
//! Every fix goes through the fakes in tests/common: nothing real changes.

mod common;

use common::*;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;

fn checks(e: &Env) -> Vec<Value> {
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    ok_data(code, &v)["checks"].as_array().unwrap().clone()
}

fn check(checks: &[Value], name: &str) -> Value {
    checks
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("no {name} in {checks:?}"))
}

fn fix_id(checks: &[Value], name: &str) -> Option<String> {
    check(checks, name)["fix"]["id"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn checks_that_pass_have_no_fix_key() {
    let e = Env::isolated();
    for c in checks(&e) {
        if c["status"] == "ok" {
            let keys: Vec<&String> = c.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["name", "status", "detail"], "{c}");
        }
    }
}

#[test]
fn every_fixable_problem_carries_its_fix_id() {
    let e = Env::isolated();
    let c = checks(&e);
    // A fresh host: no phone and no service.
    assert_eq!(fix_id(&c, "devices").as_deref(), Some("pair"));
    assert_eq!(fix_id(&c, "service").as_deref(), Some("install_service"));
    let pair = check(&c, "devices");
    assert!(pair["fix"]["description"]
        .as_str()
        .unwrap()
        .contains("falconer-hook pair"));

    e.remove_runtimes();
    e.remove_runtime(&["tailscale"]);
    e.close_ssh_port();
    let c = checks(&e);
    assert_eq!(
        fix_id(&c, "agent_runtime").as_deref(),
        Some("install_runtime")
    );
    assert_eq!(
        fix_id(&c, "tailscale").as_deref(),
        Some("install_tailscale")
    );
    assert_eq!(fix_id(&c, "ssh_port").as_deref(), Some("enable_ssh"));
    for c in &c {
        if let Some(d) = c.get("fix") {
            assert!(!d["description"].as_str().unwrap().is_empty(), "{c}");
        }
    }
}

#[test]
fn a_logged_out_tailscale_is_fixed_with_tailscale_up() {
    let e = Env::isolated();
    e.tailscale_state("NeedsLogin");
    assert_eq!(
        fix_id(&checks(&e), "tailscale").as_deref(),
        Some("tailscale_up")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_missing_openssh_server_is_fixed_by_installing_it() {
    let e = Env::isolated();
    e.close_ssh_port();
    e.remove_sshd();
    assert_eq!(
        fix_id(&checks(&e), "ssh_port").as_deref(),
        Some("install_ssh")
    );
}

#[test]
fn open_permissions_are_fixed_with_fix_permissions() {
    let e = Env::isolated();
    let cfg = e.root.join("config/falconer/config.toml");
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        fix_id(&checks(&e), "permissions").as_deref(),
        Some("fix_permissions")
    );
}

#[test]
fn human_output_never_shows_fix_ids() {
    let e = Env::isolated();
    e.remove_runtimes();
    let out = e.run(&["doctor"], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    for id in ["install_runtime", "install_service", "pair\"", "fix"] {
        assert!(!text.contains(id), "{id}: {text}");
    }
    // Same line format as before: `[mark] name detail`.
    for line in text.lines().take_while(|l| !l.is_empty()) {
        assert!(
            ["[ok  ] ", "[warn] ", "[FAIL] "]
                .iter()
                .any(|m| line.starts_with(m)),
            "{line}"
        );
    }
}

/// Interactive answers are piped; `FALCONER_ASSUME_TTY` makes stdin count as a terminal.
const TTY: &[(&str, &str)] = &[("FALCONER_ASSUME_TTY", "1")];

fn calls_to(e: &Env, prog: &str) -> Vec<String> {
    e.calls()
        .into_iter()
        .filter(|c| c[0] == prog)
        .map(|c| c.join(" "))
        .collect()
}

fn fixes(d: &Value) -> Vec<(String, String)> {
    d["fixes"]
        .as_array()
        .unwrap_or_else(|| panic!("no fixes in {d}"))
        .iter()
        .map(|f| {
            (
                f["id"].as_str().unwrap().to_string(),
                f["action"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn doctor_without_fix_keeps_its_json_shape() {
    let e = Env::isolated();
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let keys: Vec<&String> = d.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["healthy", "checks"]);
    assert!(e.calls().iter().all(|c| c[0] != "brew" && c[0] != "sudo"));
}

#[test]
fn an_unknown_fix_id_is_a_clear_error() {
    let e = Env::isolated();
    let (code, v) = e.json(&["doctor", "--fix", "bogus", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "unknown_fix");
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("bogus") && msg.contains("install_runtime"),
        "{msg}"
    );
    let out = e.run(&["doctor", "--fix", "bogus"], None, &[]);
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bogus"));
}

#[test]
fn fix_permissions_chmods_the_private_files_and_rechecks() {
    let e = Env::isolated();
    let cfg = e.root.join("config/falconer/config.toml");
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, v) = e.json(
        &["doctor", "--fix", "fix_permissions", "--yes", "--json"],
        None,
        &[],
    );
    let d = ok_data(code, &v);
    assert_eq!(fixes(d), [("fix_permissions".into(), "fixed".into())]);
    assert_eq!(mode(&cfg), 0o600);
    // The checks are run again after fixing.
    assert_eq!(
        check(d["checks"].as_array().unwrap(), "permissions")["status"],
        "ok"
    );
}

#[test]
fn fix_without_a_terminal_or_yes_only_prints() {
    let e = Env::isolated();
    e.remove_runtimes();
    let (code, v) = e.json(&["doctor", "--fix", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let f = fixes(d);
    assert!(
        f.contains(&("install_runtime".into(), "printed".into())),
        "{f:?}"
    );
    assert!(
        f.contains(&("install_service".into(), "printed".into())),
        "{f:?}"
    );
    assert!(calls_to(&e, "brew").is_empty());
    assert!(calls_to(&e, "systemctl").is_empty());
}

#[test]
fn fix_all_with_yes_applies_each_fix_once_and_never_pairs() {
    let e = Env::isolated();
    e.remove_runtimes();
    let (code, v) = e.json(&["doctor", "--fix", "--yes", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let f = fixes(d);
    let ids: Vec<&str> = f.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(ids, ["install_runtime", "pair", "install_service"], "{f:?}");
    assert_eq!(calls_to(&e, "brew"), ["brew install herdr"]);
    assert!(
        f.contains(&("install_service".into(), "installed".into())),
        "{f:?}"
    );
    assert!(
        e.root
            .join("config/systemd/user/falconer-hook.service")
            .exists()
            || cfg!(target_os = "macos")
    );
    assert!(f.contains(&("pair".into(), "manual".into())), "{f:?}");
}

#[test]
fn fix_with_an_id_applies_only_that_fix() {
    let e = Env::isolated();
    e.remove_runtimes();
    let (code, v) = e.json(
        &["doctor", "--fix", "install_service", "--yes", "--json"],
        None,
        &[],
    );
    let d = ok_data(code, &v);
    assert_eq!(fixes(d), [("install_service".into(), "installed".into())]);
    assert!(calls_to(&e, "brew").is_empty());
}

#[test]
fn a_fix_that_is_not_needed_is_reported_and_runs_nothing() {
    let e = Env::isolated();
    let (code, v) = e.json(
        &["doctor", "--fix", "install_tailscale", "--yes", "--json"],
        None,
        &[],
    );
    let d = ok_data(code, &v);
    assert_eq!(
        fixes(d),
        [("install_tailscale".into(), "not_needed".into())]
    );
    assert!(calls_to(&e, "brew").is_empty());
}

#[test]
fn interactive_fix_asks_and_declining_runs_nothing() {
    let e = Env::isolated();
    e.remove_runtimes();
    let out = e.run(&["doctor", "--fix", "install_runtime"], Some("n\nn\n"), TTY);
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Install Herdr with brew install herdr"),
        "{err}"
    );
    assert!(calls_to(&e, "brew").is_empty());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[FAIL] agent_runtime"),
        "re-ran the checks: {text}"
    );
}

#[test]
fn help_documents_fix() {
    let e = Env::isolated();
    let out = e.run(&["doctor", "--help"], None, &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--fix [<ID>]"), "{text}");
    assert!(text.contains("--yes"), "{text}");
    assert!(text.contains("install_runtime"), "{text}");
}
