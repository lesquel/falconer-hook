//! `update`: finds the latest release and reinstalls it through the
//! tag-pinned install.sh. `curl` is a fake that logs its argv and returns
//! canned output, and the "downloaded" install.sh only logs its own argv,
//! so nothing here touches the network or the real install.

mod common;

use common::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Interactive answers are piped; `FALCONER_ASSUME_TTY` makes stdin count as a terminal.
const TTY: &[(&str, &str)] = &[("FALCONER_ASSUME_TTY", "1")];
const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST_URL: &str = "https://github.com/lesquel/falconer-hook/releases/latest";

/// Fake `curl`: a HEAD request (`-I`) answers with `$FAKE_DIR/latest_url` as
/// the effective URL (or fails when `$FAKE_DIR/offline` exists); a download
/// writes `$FAKE_DIR/install.sh` to the `-o` path.
const CURL: &str = r#"#!/bin/sh
{ printf 'curl'; for a in "$@"; do printf '\037%s' "$a"; done; printf '\036'; } >> "$FAKE_LOG"
[ -f "$FAKE_DIR/offline" ] && { echo "curl: (6) Could not resolve host" >&2; exit 6; }
case " $* " in
  *" -fsSLI "*) cat "$FAKE_DIR/latest_url"; exit 0 ;;
esac
out=""; prev=""
for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
cat "$FAKE_DIR/install.sh" > "$out"
"#;

/// The "downloaded" installer: logs its argv and `FALCONER_BIN_DIR`.
const INSTALLER: &str = r#"#!/bin/sh
{ printf 'install.sh'; for a in "$@"; do printf '\037%s' "$a"; done; printf '\037BIN_DIR=%s' "$FALCONER_BIN_DIR"; printf '\037SKIP=%s' "$FALCONER_SKIP_PREREQS"; printf '\036'; } >> "$FAKE_LOG"
echo "falconer-hook installer (fake)"
exit "${FAKE_INSTALL_EXIT:-0}"
"#;

fn env_with_latest(tag: &str) -> Env {
    let e = Env::isolated();
    let curl = e.root.join("bin/curl");
    fs::write(&curl, CURL).unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    // The installer runs under the real `sh`.
    let sh = ["/bin/sh", "/usr/bin/sh"]
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .expect("sh");
    std::os::unix::fs::symlink(sh, e.root.join("tools/sh")).unwrap();
    fs::write(
        e.root.join("fake/latest_url"),
        format!("https://github.com/lesquel/falconer-hook/releases/tag/{tag}"),
    )
    .unwrap();
    fs::write(e.root.join("fake/install.sh"), INSTALLER).unwrap();
    fs::create_dir_all(e.root.join("tmp")).unwrap();
    e
}

fn next_patch() -> String {
    let mut p: Vec<u64> = CURRENT.split('.').map(|n| n.parse().unwrap()).collect();
    p[2] += 1;
    format!("v{}.{}.{}", p[0], p[1], p[2])
}

fn tmp_env(e: &Env) -> String {
    e.root.join("tmp").display().to_string()
}

fn calls_to(e: &Env, prog: &str) -> Vec<Vec<String>> {
    e.calls().into_iter().filter(|c| c[0] == prog).collect()
}

/// Curl calls that download (anything but the HEAD request).
fn downloads(e: &Env) -> Vec<Vec<String>> {
    calls_to(e, "curl")
        .into_iter()
        .filter(|c| !c.iter().any(|a| a == "-fsSLI"))
        .collect()
}

#[test]
fn check_reports_up_to_date() {
    let e = env_with_latest(&format!("v{CURRENT}"));
    let out = e.run(&["update", "--check"], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim(), format!("Up to date (v{CURRENT})"));
    let head = &calls_to(&e, "curl")[0];
    for want in ["-fsSLI", "--proto", "=https", "--tlsv1.2", LATEST_URL] {
        assert!(head.iter().any(|a| a == want), "{head:?}");
    }
    assert!(downloads(&e).is_empty());
}

#[test]
fn check_reports_an_update_in_human_and_json() {
    let next = next_patch();
    let e = env_with_latest(&next);
    let out = e.run(&["update", "--check"], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("Update available: v{CURRENT} → {next}")
    );
    let (code, v) = e.json(&["update", "--check", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["current"], CURRENT);
    assert_eq!(d["latest"], next.trim_start_matches('v'));
    assert_eq!(d["update_available"], true);
    assert!(downloads(&e).is_empty());
    assert!(calls_to(&e, "install.sh").is_empty());
}

#[test]
fn a_prerelease_is_never_offered_from_a_stable_version() {
    let e = env_with_latest("v99.0.0-rc.1");
    let (code, v) = e.json(&["update", "--check", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["latest"], "99.0.0-rc.1");
    assert_eq!(d["update_available"], false);
    let out = e.run(&["update", "--yes"], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Up to date"));
    assert!(downloads(&e).is_empty());
    assert!(calls_to(&e, "install.sh").is_empty());
}

#[test]
fn declining_changes_nothing() {
    let e = env_with_latest(&next_patch());
    let mut env = TTY.to_vec();
    let tmp = tmp_env(&e);
    env.push(("TMPDIR", &tmp));
    let out = e.run(&["update"], Some("n\n"), &env);
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    let url = format!(
        "https://raw.githubusercontent.com/lesquel/falconer-hook/{}/install.sh",
        next_patch()
    );
    assert!(err.contains(&url), "{err}");
    assert!(err.contains("[y/N]"), "{err}");
    assert!(downloads(&e).is_empty());
    assert!(calls_to(&e, "install.sh").is_empty());
    assert!(String::from_utf8_lossy(&out.stdout).contains("declined"));
    assert_eq!(fs::read_dir(e.root.join("tmp")).unwrap().count(), 0);
}

#[test]
fn yes_runs_the_tag_pinned_installer() {
    let next = next_patch();
    let e = env_with_latest(&next);
    let tmp = tmp_env(&e);
    let (code, v) = e.json(&["update", "--yes", "--json"], None, &[("TMPDIR", &tmp)]);
    let d = ok_data(code, &v);
    assert_eq!(d["action"], "updated");
    assert_eq!(d["tag"], next);

    let dl = downloads(&e);
    assert_eq!(dl.len(), 1, "{dl:?}");
    let url = format!("https://raw.githubusercontent.com/lesquel/falconer-hook/{next}/install.sh");
    for want in ["--proto", "=https", "--tlsv1.2", url.as_str()] {
        assert!(dl[0].iter().any(|a| a == want), "{:?}", dl[0]);
    }
    let run = &calls_to(&e, "install.sh");
    assert_eq!(run.len(), 1, "{run:?}");
    // No unit file: the installer must not register a service.
    let bin_dir = Path::new(BIN).canonicalize().unwrap();
    let bin_dir = bin_dir.parent().unwrap().display().to_string();
    assert_eq!(
        run[0][1..],
        [
            "--version".to_string(),
            next.clone(),
            "--no-service".to_string(),
            format!("BIN_DIR={bin_dir}"),
            "SKIP=1".to_string(),
        ]
    );
    // The private temp copy is gone.
    assert_eq!(fs::read_dir(e.root.join("tmp")).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn yes_keeps_and_restarts_an_installed_service() {
    let e = env_with_latest(&next_patch());
    let unit = e.root.join("config/systemd/user/falconer-hook.service");
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, "[Unit]\n").unwrap();
    let tmp = tmp_env(&e);
    let (code, v) = e.json(&["update", "--yes", "--json"], None, &[("TMPDIR", &tmp)]);
    ok_data(code, &v);
    let run = &calls_to(&e, "install.sh")[0];
    assert!(!run.iter().any(|a| a == "--no-service"), "{run:?}");
    let restarts: Vec<String> = calls_to(&e, "systemctl")
        .into_iter()
        .map(|c| c[1..].join(" "))
        .collect();
    assert_eq!(restarts, ["--user restart falconer-hook.service"]);
}

#[test]
fn version_pins_the_tag_without_resolving() {
    let e = env_with_latest("v0.0.1");
    let tmp = tmp_env(&e);
    let (code, v) = e.json(
        &["update", "--version", "v0.1.0", "--yes", "--json"],
        None,
        &[("TMPDIR", &tmp)],
    );
    assert_eq!(ok_data(code, &v)["tag"], "v0.1.0");
    assert!(!calls_to(&e, "curl")
        .iter()
        .any(|c| c.iter().any(|a| a == "-fsSLI")));
    assert_eq!(calls_to(&e, "install.sh")[0][2], "v0.1.0");
    let (code, v) = e.json(
        &["update", "--version", "1.0; rm -rf ~", "--json"],
        None,
        &[],
    );
    assert_eq!(err_code(code, &v), "invalid_input");
}

#[test]
fn without_a_terminal_it_only_prints() {
    let next = next_patch();
    let e = env_with_latest(&next);
    let out = e.run(&["update"], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("Update available: v{CURRENT} → {next}")),
        "{text}"
    );
    assert!(text.contains("install.sh --version"), "{text}");
    let (code, v) = e.json(&["update", "--json"], None, TTY);
    let d = ok_data(code, &v);
    assert_eq!(d["action"], "printed");
    assert!(d["command"].as_str().unwrap().contains("--version"), "{d}");
    assert!(downloads(&e).is_empty());
    assert!(calls_to(&e, "install.sh").is_empty());
}

#[test]
fn a_failing_installer_is_an_error() {
    let e = env_with_latest(&next_patch());
    let tmp = tmp_env(&e);
    let (code, v) = e.json(
        &["update", "--yes", "--json"],
        None,
        &[("TMPDIR", &tmp), ("FAKE_INSTALL_EXIT", "3")],
    );
    assert_eq!(err_code(code, &v), "update_failed");
    assert_eq!(fs::read_dir(e.root.join("tmp")).unwrap().count(), 0);
}

#[test]
fn an_unresolvable_release_is_an_error_and_changes_nothing() {
    let e = env_with_latest("v0.0.0");
    fs::write(e.root.join("fake/offline"), "").unwrap();
    let (code, v) = e.json(&["update", "--check", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "unavailable");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("could not resolve the latest release"));
    let out = e.run(&["update", "--yes"], None, &[]);
    assert_ne!(out.status.code(), Some(0));
    assert!(calls_to(&e, "install.sh").is_empty());

    // No redirect to a tag (e.g. no release yet) is the same error.
    let e = env_with_latest("v0.0.0");
    fs::write(e.root.join("fake/latest_url"), LATEST_URL).unwrap();
    let (code, v) = e.json(&["update", "--check", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "unavailable");

    // Neither curl nor wget.
    let e = env_with_latest("v0.0.0");
    e.remove_tool("curl");
    let (code, v) = e.json(&["update", "--check", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "unavailable");
}
