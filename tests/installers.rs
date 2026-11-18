//! `setup` installing Herdr and Tailscale with their official installers,
//! the OpenSSH server through the package manager, and `tailscale up`.
//! `curl`, `wget`, `sh`, the package managers and `sudo` are fakes that only
//! log their argv: nothing here touches the network or the system.

mod common;

use common::*;
use serde_json::Value;
use std::path::Path;

/// Interactive answers are piped; `FALCONER_ASSUME_TTY` makes stdin count as a terminal.
const TTY: &[(&str, &str)] = &[("FALCONER_ASSUME_TTY", "1")];

const HERDR_URL: &str = "https://herdr.dev/install.sh";
const TAILSCALE_URL: &str = "https://tailscale.com/install.sh";

fn calls_to(e: &Env, prog: &str) -> Vec<Vec<String>> {
    e.calls().into_iter().filter(|c| c[0] == prog).collect()
}

fn joined(e: &Env, prog: &str) -> Vec<String> {
    calls_to(e, prog).into_iter().map(|c| c.join(" ")).collect()
}

fn prereq<'a>(d: &'a Value, key: &str) -> &'a Value {
    &d["prerequisites"][key]
}

/// No runtime, no Homebrew, apt and curl present.
fn apt_with_curl() -> Env {
    let e = Env::isolated();
    e.remove_runtimes();
    e.remove_tool("brew");
    e.add_tool("curl");
    e.add_tool("sh");
    e
}

fn tmp_is_empty(e: &Env) -> bool {
    std::fs::read_dir(e.tmp()).unwrap().next().is_none()
}

#[test]
fn herdr_official_installer_runs_from_a_private_file_with_yes() {
    let e = apt_with_curl();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let r = prereq(d, "agent_runtime");
    assert_eq!(r["action"], "installed", "{r}");
    assert_eq!(r["runtime"], "herdr");
    assert_eq!(r["url"], HERDR_URL);
    let curl = calls_to(&e, "curl");
    assert_eq!(curl.len(), 1, "{curl:?}");
    let file = curl[0][6].clone();
    assert_eq!(
        curl[0],
        [
            "curl",
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "-o",
            file.as_str(),
            HERDR_URL
        ]
    );
    assert!(file.starts_with(&e.tmp().display().to_string()), "{file}");
    assert_eq!(joined(&e, "sh"), [format!("sh {file}")]);
    // The installer existed when it ran, and is gone afterwards.
    assert!(std::fs::read_to_string(e.root.join("fake/sh_ran"))
        .unwrap()
        .contains("fake installer"));
    assert!(!Path::new(&file).exists());
    assert!(tmp_is_empty(&e));
    // Herdr worked, so tmux was not installed.
    assert!(joined(&e, "sudo").is_empty(), "{:?}", joined(&e, "sudo"));
}

#[test]
fn herdr_installer_shows_the_url_and_command_and_declining_offers_tmux() {
    let e = apt_with_curl();
    let out = e.run(&["setup", "--no-service"], Some("n\ny\nn\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(HERDR_URL), "{err}");
    assert!(
        err.contains("curl --proto =https --tlsv1.2 -fsSL -o"),
        "{err}"
    );
    assert!(err.contains("&& sh "), "{err}");
    assert!(
        err.contains("Install tmux with sudo apt-get install -y tmux? [y/N]"),
        "{err}"
    );
    assert!(calls_to(&e, "curl").is_empty());
    assert!(calls_to(&e, "sh").is_empty());
    assert_eq!(joined(&e, "sudo"), ["sudo apt-get install -y tmux"]);
}

#[test]
fn herdr_installer_is_only_printed_without_a_terminal() {
    let e = apt_with_curl();
    let out = e.run(&["setup", "--no-service"], None, &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(HERDR_URL), "{text}");
    assert!(calls_to(&e, "curl").is_empty());
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let r = prereq(d, "agent_runtime");
    assert_eq!(r["action"], "printed");
    assert_eq!(r["herdr"]["action"], "printed");
    assert_eq!(r["herdr"]["url"], HERDR_URL);
    assert!(calls_to(&e, "curl").is_empty());
    assert!(calls_to(&e, "sh").is_empty());
    assert!(tmp_is_empty(&e));
}

#[test]
fn wget_is_used_when_curl_is_missing() {
    let e = apt_with_curl();
    e.remove_tool("curl");
    e.add_tool("wget");
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    ok_data(code, &v);
    let wget = calls_to(&e, "wget");
    assert_eq!(wget.len(), 1, "{wget:?}");
    assert_eq!(
        wget[0][..4],
        ["wget", "--https-only", "--secure-protocol=TLSv1_2", "-q"]
    );
    assert_eq!(wget[0][4], "-O");
    assert_eq!(wget[0][6], HERDR_URL);
    assert_eq!(joined(&e, "sh"), [format!("sh {}", wget[0][5])]);
}

#[test]
fn homebrew_stays_preferred_over_the_herdr_installer() {
    let e = Env::isolated();
    e.remove_runtimes();
    e.add_tool("curl");
    e.add_tool("sh");
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(prereq(d, "agent_runtime")["command"], "brew install herdr");
    assert_eq!(joined(&e, "brew"), ["brew install herdr"]);
    assert!(calls_to(&e, "curl").is_empty());
}

#[test]
fn tailscale_is_already_running_by_default() {
    let e = Env::isolated();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(prereq(d, "tailscale")["action"], "already_running");
    let keys: Vec<&String> = d["prerequisites"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["agent_runtime", "ssh", "tailscale"]);
}

#[test]
fn missing_tailscale_uses_homebrew_when_present() {
    let e = Env::isolated();
    e.remove_runtime(&["tailscale"]);
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let t = prereq(d, "tailscale");
    assert_eq!(t["action"], "installed", "{t}");
    assert_eq!(t["command"], "brew install tailscale");
    assert_eq!(joined(&e, "brew"), ["brew install tailscale"]);
}

#[test]
fn missing_tailscale_uses_its_official_installer_without_homebrew() {
    let e = Env::isolated();
    e.remove_runtime(&["tailscale"]);
    e.remove_tool("brew");
    e.add_tool("curl");
    e.add_tool("sh");
    // Declined at the prompt: nothing downloaded.
    let out = e.run(&["setup", "--no-service"], Some("n\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(TAILSCALE_URL), "{err}");
    assert!(calls_to(&e, "curl").is_empty());
    // Without a terminal: printed.
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    assert_eq!(prereq(ok_data(code, &v), "tailscale")["action"], "printed");
    assert!(calls_to(&e, "curl").is_empty());
    // --yes: downloaded to a private file, run with sh, deleted.
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let t = prereq(ok_data(code, &v), "tailscale").clone();
    assert_eq!(t["action"], "installed", "{t}");
    assert_eq!(t["url"], TAILSCALE_URL);
    let curl = calls_to(&e, "curl");
    assert_eq!(curl.len(), 1);
    assert_eq!(curl[0][7], TAILSCALE_URL);
    assert_eq!(joined(&e, "sh"), [format!("sh {}", curl[0][6])]);
    assert!(tmp_is_empty(&e));
}

#[test]
fn logged_out_tailscale_is_brought_up_after_consent() {
    let e = Env::isolated();
    e.tailscale_state("NeedsLogin");
    let ts = e.root.join("bin/tailscale").display().to_string();
    // Declined: nothing runs.
    let out = e.run(&["setup", "--no-service"], Some("n\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("sudo {ts} up")), "{err}");
    assert!(joined(&e, "sudo").is_empty());
    // Accepted: run on the terminal so the login URL shows.
    e.run(&["setup", "--no-service"], Some("y\n"), TTY);
    assert_eq!(joined(&e, "sudo"), [format!("sudo {ts} up")]);
    // --yes without a person to log in: printed only.
    let e = Env::isolated();
    e.tailscale_state("NeedsLogin");
    let ts = e.root.join("bin/tailscale").display().to_string();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let t = prereq(ok_data(code, &v), "tailscale").clone();
    assert_eq!(t["action"], "printed", "{t}");
    assert_eq!(t["command"], format!("sudo {ts} up"));
    assert!(joined(&e, "sudo").is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn missing_openssh_server_is_installed_before_it_is_enabled() {
    let e = Env::isolated();
    e.remove_tool("brew");
    e.close_ssh_port();
    e.remove_sshd();
    // Declined: neither installed nor enabled.
    let out = e.run(&["setup", "--no-service"], Some("n\n"), TTY);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Install the OpenSSH server with sudo apt-get install -y openssh-server?"),
        "{err}"
    );
    assert!(joined(&e, "sudo").is_empty());
    // Without a terminal: printed.
    let (code, v) = e.json(&["setup", "--no-service", "--json"], None, &[]);
    let s = prereq(ok_data(code, &v), "ssh").clone();
    assert_eq!(s["action"], "printed", "{s}");
    assert_eq!(s["command"], "sudo apt-get install -y openssh-server");
    assert_eq!(s["enable"], "sudo systemctl enable --now ssh");
    // --yes: install, then enable as before.
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let s = prereq(ok_data(code, &v), "ssh").clone();
    assert_eq!(s["install"]["action"], "installed", "{s}");
    assert_eq!(
        joined(&e, "sudo"),
        [
            "sudo apt-get install -y openssh-server",
            "sudo systemctl enable --now ssh"
        ]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_installed_openssh_server_is_only_enabled() {
    let e = Env::isolated();
    e.remove_tool("brew");
    e.close_ssh_port();
    let (code, v) = e.json(&["setup", "--yes", "--no-service", "--json"], None, &[]);
    let s = prereq(ok_data(code, &v), "ssh").clone();
    assert!(s.get("install").is_none(), "{s}");
    assert_eq!(joined(&e, "sudo"), ["sudo systemctl enable --now ssh"]);
}
