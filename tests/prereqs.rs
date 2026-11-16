//! `setup` fixing prerequisites (tmux, the SSH server) and offering to pair,
//! plus the doctor SSH-port check. Package managers and `sudo` are fakes that
//! only log their argv, so nothing here installs anything.

mod common;

use common::*;
use serde_json::Value;

/// Calls to a fake program, as space-joined argv.
fn calls_to(e: &Env, prog: &str) -> Vec<String> {
    e.calls()
        .into_iter()
        .filter(|c| c[0] == prog)
        .map(|c| c.join(" "))
        .collect()
}

fn runtime(d: &Value) -> &Value {
    &d["prerequisites"]["agent_runtime"]
}

/// Interactive answers are piped; `FALCONER_ASSUME_TTY` makes stdin count as a terminal.
const TTY: &[(&str, &str)] = &[("FALCONER_ASSUME_TTY", "1")];

fn no_tmux() -> Env {
    let e = Env::isolated();
    e.remove_runtimes();
    e
}

#[test]
fn herdr_is_offered_first_with_brew_and_accepted() {
    let e = no_tmux();
    let out = e.run(&["setup", "--no-service"], Some("y\nn\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "Install Herdr with brew install herdr (unlocks View as agents in the app)? [y/N]"
        ),
        "{err}"
    );
    assert!(!err.contains("Install tmux"), "{err}");
    assert_eq!(calls_to(&e, "brew"), ["brew install herdr"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Installed Herdr"));
}

#[test]
fn declining_herdr_offers_tmux_next() {
    let e = no_tmux();
    let out = e.run(&["setup", "--no-service"], Some("n\ny\nn\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Install tmux with brew install tmux? [y/N]"),
        "{err}"
    );
    assert_eq!(calls_to(&e, "brew"), ["brew install tmux"]);
}

#[test]
fn declining_both_runs_nothing() {
    let e = no_tmux();
    let out = e.run(&["setup", "--no-service"], Some("n\nn\n"), TTY);
    assert!(calls_to(&e, "brew").is_empty());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("No agent runtime installed (declined)"),
        "{text}"
    );
}

#[test]
fn yes_installs_herdr_without_asking() {
    let e = no_tmux();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(runtime(d)["action"], "installed");
    assert_eq!(runtime(d)["runtime"], "herdr");
    assert_eq!(runtime(d)["command"], "brew install herdr");
    assert_eq!(calls_to(&e, "brew"), ["brew install herdr"]);
}

#[test]
fn without_a_terminal_only_prints_the_commands() {
    let e = no_tmux();
    let out = e.run(&["setup", "--no-service"], None, &[]);
    assert!(calls_to(&e, "brew").is_empty());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Install Herdr with: brew install herdr"),
        "{text}"
    );
    assert!(text.contains("or tmux with: brew install tmux"), "{text}");
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(runtime(d)["action"], "printed");
    assert_eq!(runtime(d)["command"], "brew install tmux");
    assert_eq!(runtime(d)["herdr"]["command"], "brew install herdr");
    assert!(calls_to(&e, "brew").is_empty());
}

#[test]
fn tmux_only_offers_herdr_interactively() {
    let e = Env::isolated();
    e.remove_runtime(&["herdr"]);
    // Interactive: offered with the reason; declining changes nothing.
    let out = e.run(&["setup", "--no-service"], Some("n\nn\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unlocks View as agents in the app"), "{err}");
    assert!(calls_to(&e, "brew").is_empty());
    // Accepting installs Herdr.
    e.run(&["setup", "--no-service"], Some("y\nn\n"), TTY);
    assert_eq!(calls_to(&e, "brew"), ["brew install herdr"]);
    // --yes and no terminal never install an optional runtime.
    let e = Env::isolated();
    e.remove_runtime(&["herdr"]);
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    assert_eq!(runtime(ok_data(code, &v))["runtime"], "tmux");
    e.run(&["setup", "--no-service"], None, &[]);
    assert!(calls_to(&e, "brew").is_empty());
}

#[test]
fn doctor_points_at_brew_for_herdr_only_when_brew_exists() {
    let e = Env::isolated();
    e.remove_runtime(&["herdr"]);
    let detail = |e: &Env| {
        let (_, v) = e.json(&["doctor", "--json"], None, &[]);
        v["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "herdr")
            .unwrap()["detail"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(detail(&e).contains("`brew install herdr`"));
    e.remove_tool("brew");
    assert_eq!(detail(&e), "not installed");
}

#[test]
fn other_package_managers_install_tmux_through_sudo_and_only_mention_herdr() {
    let e = no_tmux();
    e.remove_tool("brew");
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(runtime(d)["command"], "sudo apt-get install -y tmux");
    assert_eq!(runtime(d)["alternative"], "https://herdr.dev");
    assert_eq!(calls_to(&e, "sudo"), ["sudo apt-get install -y tmux"]);
    assert!(calls_to(&e, "apt-get").is_empty(), "only sudo is spawned");
}

#[test]
fn no_package_manager_prints_guidance() {
    let e = no_tmux();
    e.remove_tool("brew");
    e.remove_tool("apt-get");
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(runtime(d)["action"], "unavailable");
    assert!(calls_to(&e, "sudo").is_empty());
}

#[test]
fn nothing_is_installed_when_a_runtime_exists() {
    let e = Env::isolated();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(runtime(d)["action"], "already_installed");
    assert_eq!(runtime(d)["runtime"], "herdr");
    assert!(calls_to(&e, "brew").is_empty());
}

#[test]
fn doctor_fails_when_nothing_listens_on_the_ssh_port() {
    let e = Env::isolated();
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let check = |v: &Value| {
        v["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "ssh_port")
            .cloned()
            .unwrap()
    };
    assert_eq!(code, 0);
    assert_eq!(check(&v)["status"], "ok");
    e.close_ssh_port();
    let (_, v) = e.json(&["doctor", "--json"], None, &[]);
    let c = check(&v);
    assert_eq!(c["status"], "fail");
    let detail = c["detail"].as_str().unwrap();
    assert!(detail.contains("nothing listens"), "{detail}");
    assert!(detail.contains("Remote Login") || detail.contains("systemctl enable --now"));
    assert_eq!(v["data"]["healthy"], false);
}

#[test]
fn setup_offers_to_turn_ssh_on_and_follows_the_same_rules() {
    // Without a terminal: printed only.
    let e = Env::isolated();
    e.close_ssh_port();
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["prerequisites"]["ssh"]["action"], "printed");
    assert!(calls_to(&e, "sudo").is_empty());
    // Declined at the prompt.
    let out = e.run(&["setup", "--no-service"], Some("n\n"), TTY);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Turn it on with sudo"));
    assert!(calls_to(&e, "sudo").is_empty());
    // --yes runs the command (the fake does not open the port, so it reports failure).
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["prerequisites"]["ssh"]["action"], "failed");
    let ran = calls_to(&e, "sudo");
    assert_eq!(ran.len(), 1, "{ran:?}");
    assert!(
        ran[0].starts_with("sudo systemsetup -setremotelogin on")
            || ran[0].starts_with("sudo systemctl enable --now")
    );
}

#[test]
fn setup_asks_to_pair_only_when_a_person_can_scan() {
    let e = Env::isolated();
    // --yes never pairs, it prints the next step.
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["pairing"]["action"], "skipped");
    assert_eq!(d["pairing"]["reason"], "needs_a_person");
    // Without a terminal: next step only.
    let out = e.run(&["setup", "--no-service"], None, &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Next: run `falconer-hook pair`"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Pair a phone now?"));
    // Interactive: asked, and "n" leaves it for later.
    let out = e.run(&["setup", "--no-service"], Some("n\n"), TTY);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Pair a phone now? [Y/n]"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Next: run `falconer-hook pair`"));
}

#[test]
fn setup_runs_the_pair_flow_when_accepted() {
    let e = Env::isolated();
    // stdin is a pipe, so pair itself refuses without --yes: proves it was invoked.
    let out = e.run(&["setup", "--no-service"], Some("\n"), TTY);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Pairing did not finish"), "{text}");
}
