//! End-to-end CLI tests: the real binary against a temporary HOME/XDG tree
//! and fake `herdr`, `tmux`, `tailscale` and `systemctl` scripts that only
//! log their argv. Nothing here touches a real pane, the real
//! `authorized_keys`, Claude settings or systemd.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_openmosh-hook");
const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH root@host";
const AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"blocked","cwd":"/w","pane_id":"w7:pA","state_change_seq":21,"terminal_title_stripped":"Fix tests","workspace_id":"w7"}],"type":"agent_list"}}"#;

const FAKE: &str = r#"#!/bin/sh
name=$(basename "$0")
{ printf '%s' "$name"; for a in "$@"; do printf '\037%s' "$a"; done; printf '\036'; } >> "$FAKE_LOG"
case "$name $1 $2" in
  "herdr agent list") cat "$FAKE_DIR/agents.json" ;;
  "herdr  "|"herdr session attach")
    # Interactive client stand-in: wait for the test to release it, then exit 3.
    i=0; while [ ! -f "$FAKE_DIR/release" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done
    exit 3 ;;
  "herdr session list") echo '{"sessions":[{"default":true,"name":"default","running":true}]}' ;;
  "tmux list-sessions"*) printf 'main\t1\n' ;;
  "tailscale status --json") cat "$FAKE_DIR/ts_status.json" ;;
  "tailscale ip -4") cat "$FAKE_DIR/ts_ip" ;;
  "tailscale whois --json") echo '{"Node":{},"UserProfile":{"LoginName":"me@example.com"}}' ;;
  "systemctl --user is-active") echo inactive; exit 3 ;;
esac
exit 0
"#;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for name in ["herdr", "tmux", "tailscale", "systemctl", "launchctl"] {
            let p = bin.join(name);
            fs::write(&p, FAKE).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fake = root.join("fake");
        fs::create_dir_all(&fake).unwrap();
        fs::write(fake.join("agents.json"), AGENTS).unwrap();
        fs::write(
            fake.join("ts_status.json"),
            r#"{"BackendState":"Running","Self":{"UserID":1,"DNSName":"box.tail.ts.net."},"User":{"1":{"LoginName":"me@example.com"}}}"#,
        )
        .unwrap();
        fs::write(fake.join("ts_ip"), "100.88.77.66\n").unwrap();
        fs::write(root.join("ssh_host_ed25519_key.pub"), HOST_KEY).unwrap();
        let cfg_dir = root.join("config/openmosh");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::set_permissions(&cfg_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let b = |n: &str| bin.join(n).display().to_string();
        let cfg = format!(
            "host_alias = \"devbox\"\nssh_host_key_pub = \"{}\"\n[bins]\nherdr = \"{}\"\ntmux = \"{}\"\ntailscale = \"{}\"\nsystemctl = \"{}\"\nlaunchctl = \"{}\"\n",
            root.join("ssh_host_ed25519_key.pub").display(),
            b("herdr"),
            b("tmux"),
            b("tailscale"),
            b("systemctl"),
            b("launchctl")
        );
        let cfg_file = cfg_dir.join("config.toml");
        fs::write(&cfg_file, cfg).unwrap();
        fs::set_permissions(&cfg_file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state/openmosh")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("USER", "tester")
            .env("FAKE_LOG", self.root.join("calls.log"))
            .env("FAKE_DIR", self.root.join("fake"));
        c
    }

    fn run(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
        let mut c = self.cmd(args);
        for (k, v) in extra_env {
            c.env(k, v);
        }
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        if let Some(s) = stdin {
            child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }

    /// Runs with `--json` semantics and returns (exit code, envelope).
    fn json(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> (i32, Value) {
        let out = self.run(args, stdin, extra_env);
        let stdout = String::from_utf8(out.stdout).unwrap();
        let v: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| {
            panic!(
                "not JSON: {stdout:?} stderr={:?}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), v)
    }

    /// Fake-program invocations, each as its argv.
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .split('\u{1e}')
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\u{1f}').map(str::to_string).collect())
            .collect()
    }
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn ok_data(code: i32, v: &Value) -> &Value {
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    &v["data"]
}

fn err_code(code: i32, v: &Value) -> String {
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"]["message"].is_string());
    v["error"]["code"].as_str().unwrap().to_string()
}

const NOTIFICATION: &str = r#"{"session_id":"s","transcript_path":"/x/t.jsonl","cwd":"/w","hook_event_name":"Notification","message":"Claude needs your permission to use Bash"}"#;

#[test]
fn version_envelope() {
    let e = Env::new();
    let (code, v) = e.json(&["version", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["protocol"], json!({"pair": 1, "push": 1}));
    let raw = String::from_utf8(e.run(&["version", "--json"], None, &[]).stdout).unwrap();
    assert!(
        raw.starts_with(r#"{"ok":true,"data":"#),
        "envelope key order: {raw}"
    );
}

#[test]
fn first_run_creates_private_state() {
    let e = Env::new();
    let (code, v) = e.json(&["inbox", "list", "--json"], None, &[]);
    assert_eq!(ok_data(code, &v), &json!({"events": [], "last_seq": 0}));
    assert_eq!(mode(&e.state()), 0o700);
    assert_eq!(mode(&e.state().join("host.key")), 0o600);
    assert_eq!(mode(&e.state().join("events.db")), 0o600);
}

#[test]
fn loose_state_dir_is_refused() {
    let e = Env::new();
    e.json(&["inbox", "list", "--json"], None, &[]);
    fs::set_permissions(e.state(), fs::Permissions::from_mode(0o755)).unwrap();
    let (code, v) = e.json(&["inbox", "list", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "permissions_too_open");
    fs::set_permissions(e.state(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(
        e.state().join("host.key"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let (code, v) = e.json(&["devices", "list", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "permissions_too_open");
}

#[test]
fn emit_records_from_tmux_and_respond_delivers_keys() {
    let e = Env::new();
    let tmux = [("TMUX_PANE", "%5")];
    let (code, v) = e.json(
        &["emit", "--source", "claude", "--json"],
        Some(NOTIFICATION),
        &tmux,
    );
    let d = ok_data(code, &v).clone();
    assert_eq!(d["recorded"], true);
    assert_eq!(d["pushed"], 0, "no devices yet");
    let (code, v) = e.json(
        &["emit", "--source", "claude", "--json"],
        Some(NOTIFICATION),
        &tmux,
    );
    assert_eq!(ok_data(code, &v)["duplicate"], true);

    let (code, v) = e.json(&["inbox", "list", "--json"], None, &[]);
    let ev = &ok_data(code, &v)["events"][0];
    assert_eq!(ev["kind"], "approval_required");
    assert_eq!(
        (ev["pane"].as_str(), ev["runtime"].as_str()),
        (Some("%5"), Some("tmux"))
    );
    assert_eq!(ev["summary"], "Claude needs your permission to use Bash");
    let id = ev["id"].as_str().unwrap().to_string();

    let (code, v) = e.json(&["respond", &id, "--action", "allow", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(
        (d["event_id"].as_str(), d["action"].as_str()),
        (Some(id.as_str()), Some("allow"))
    );
    let sent: Vec<_> = e.calls().into_iter().filter(|c| c[0] == "tmux").collect();
    assert_eq!(sent, vec![vec!["tmux", "send-keys", "-t", "%5", "Enter"]]);

    let (code, v) = e.json(&["respond", &id, "--action", "allow", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "already_resolved");
    let (code, v) = e.json(&["inbox", "list", "--json", "--since", "1"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["events"][0]["status"], "resolved");
    assert_eq!(d["events"][0]["resolution"], "allow");
}

#[test]
fn respond_text_from_herdr_pane_uses_one_argv_entry() {
    let e = Env::new();
    let herdr = [("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w7:pA")];
    let idle = r#"{"hook_event_name":"Notification","message":"Claude is waiting for your input"}"#;
    let (code, v) = e.json(
        &["emit", "--source", "claude", "--json"],
        Some(idle),
        &herdr,
    );
    let id = ok_data(code, &v)["event_id"].as_str().unwrap().to_string();
    let text = "-n; $(rm -rf ~) `x`\u{1b}[2J\nnext";
    let (code, v) = e.json(
        &["respond", &id, "--action", "text", "--text", text, "--json"],
        None,
        &[],
    );
    ok_data(code, &v);
    let sent: Vec<_> = e.calls().into_iter().filter(|c| c[0] == "herdr").collect();
    assert_eq!(
        sent,
        vec![
            vec![
                "herdr",
                "pane",
                "send-text",
                "--",
                "w7:pA",
                "-n; $(rm -rf ~) `x`[2J next"
            ],
            vec!["herdr", "agent", "send-keys", "w7:pA", "enter"],
        ]
    );
}

#[test]
fn ids_starting_with_a_hyphen_are_accepted() {
    let e = Env::new();
    for args in [
        vec![
            "respond",
            "-AAAAAAAAAAAAAAAAAAAAA",
            "--action",
            "deny",
            "--json",
        ],
        vec!["devices", "revoke", "-AAAAAAAAAAAAAAAAAAAAA", "--json"],
        vec![
            "devices",
            "set-endpoint",
            "-AAAAAAAAAAAAAAAAAAAAA",
            "https://ntfy.sh/x",
            "--json",
        ],
    ] {
        let (code, v) = e.json(&args, None, &[]);
        assert_eq!(err_code(code, &v), "not_found", "{args:?}");
    }
}

#[test]
fn respond_errors() {
    let e = Env::new();
    let (code, v) = e.json(
        &[
            "respond",
            "AAAAAAAAAAAAAAAAAAAAAA",
            "--action",
            "deny",
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(err_code(code, &v), "not_found");
    let (code, v) = e.json(
        &["emit", "--source", "claude", "--json"],
        Some(NOTIFICATION),
        &[],
    );
    let id = ok_data(code, &v)["event_id"].as_str().unwrap().to_string();
    let (code, v) = e.json(&["respond", &id, "--action", "allow", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "no_pane");
    let long = "a".repeat(4097);
    let (code, v) = e.json(
        &[
            "respond", &id, "--action", "text", "--text", &long, "--json",
        ],
        None,
        &[],
    );
    assert_eq!(err_code(code, &v), "text_too_long");
    let (code, v) = e.json(
        &["emit", "--source", "claude", "--json"],
        Some("not json"),
        &[],
    );
    assert_eq!(err_code(code, &v), "invalid_input");
}

#[test]
fn agents_and_sessions_list() {
    let e = Env::new();
    let (code, v) = e.json(&["agents", "list", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["agents"][0]["pane_id"], "w7:pA");
    assert_eq!(d["agents"][0]["status"], "blocked");
    assert_eq!(d["unavailable"], json!([]));
    let (code, v) = e.json(&["sessions", "list", "--json"], None, &[]);
    let names: Vec<_> = ok_data(code, &v)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            format!(
                "{}:{}",
                s["runtime"].as_str().unwrap(),
                s["name"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(names, vec!["herdr:default", "tmux:main"]);
}

#[test]
fn pair_ssh_then_devices_lifecycle() {
    let e = Env::new();
    let ppk = "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw";
    let req =
        format!(r#"{{"name":"Pixel 8","ppk":"{ppk}","endpoint":"https://ntfy.sh/upTest?up=1"}}"#);
    let (code, v) = e.json(&["pair-ssh", "--json"], Some(&req), &[]);
    let d = ok_data(code, &v).clone();
    assert_eq!(d["alias"], "devbox");
    assert_eq!(d["hpk"].as_str().unwrap().len(), 43);
    let id = d["device_id"].as_str().unwrap().to_string();
    assert_eq!(mode(&e.state().join("devices.toml")), 0o600);
    assert!(
        !e.home().join(".ssh/authorized_keys").exists(),
        "SSH pairing adds no key"
    );

    let (code, v) = e.json(&["devices", "list", "--json"], None, &[]);
    let list = &ok_data(code, &v)["devices"];
    assert_eq!(list[0]["id"], id.as_str());
    assert!(list[0].get("ppk").is_none(), "push key is not listed");

    let (code, v) = e.json(
        &["devices", "set-endpoint", &id, "http://ntfy.sh/x", "--json"],
        None,
        &[],
    );
    assert_eq!(err_code(code, &v), "invalid_endpoint");
    let (code, v) = e.json(
        &[
            "devices",
            "set-endpoint",
            &id,
            "http://100.101.1.2:2586/up",
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(
        ok_data(code, &v)["device"]["endpoint"],
        "http://100.101.1.2:2586/up"
    );

    let (code, v) = e.json(&["devices", "revoke", &id, "--json"], None, &[]);
    assert_eq!(ok_data(code, &v)["revoked"]["id"], id.as_str());
    let (code, v) = e.json(&["devices", "revoke", &id, "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "not_found");

    let (code, v) = e.json(
        &["pair-ssh", "--json"],
        Some(r#"{"name":"x","ppk":"bad","endpoint":"https://a"}"#),
        &[],
    );
    assert_eq!(err_code(code, &v), "invalid_input");
}

#[test]
fn claude_integration_round_trip() {
    let e = Env::new();
    let settings = e.root.join("settings.json");
    let original = "{\n  \"model\": \"opus\",\n  \"permissions\": {\"allow\": [\"Bash(ls)\"]}\n}\n";
    fs::write(&settings, original).unwrap();
    let s = settings.to_str().unwrap();
    let (code, v) = e.json(
        &[
            "integration",
            "install",
            "claude",
            "--settings",
            s,
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(ok_data(code, &v)["changed"], true);
    let (code, v) = e.json(
        &[
            "integration",
            "install",
            "claude",
            "--settings",
            s,
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(ok_data(code, &v)["changed"], false, "idempotent");
    let installed: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(installed["model"], "opus");
    assert_eq!(installed["permissions"]["allow"][0], "Bash(ls)");
    for ev in ["Notification", "Stop"] {
        let cmd = installed["hooks"][ev][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            cmd.ends_with("openmosh-hook' emit --source claude"),
            "{cmd}"
        );
    }
    let (code, v) = e.json(
        &[
            "integration",
            "uninstall",
            "claude",
            "--settings",
            s,
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(ok_data(code, &v)["changed"], true);
    let restored: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored, serde_json::from_str::<Value>(original).unwrap());
    assert!(
        !e.home().join(".claude").exists(),
        "default settings path untouched"
    );
}

#[test]
fn service_dry_run_writes_nothing() {
    let e = Env::new();
    let (code, v) = e.json(&["service", "install", "--dry-run", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["dry_run"], true);
    assert!(d["contents"].as_str().unwrap().contains("serve\n"));
    assert!(!e.root.join("config/systemd").exists());
    assert!(e.calls().is_empty(), "no command ran");
}

#[test]
fn doctor_reports_checks() {
    let e = Env::new();
    let (code, v) = e.json(&["doctor", "--json"], None, &[]);
    let d = ok_data(code, &v);
    let names: Vec<_> = d["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    for n in [
        "config",
        "tailscale",
        "herdr",
        "tmux",
        "permissions",
        "sshd",
        "devices",
        "keymaps",
        "service",
    ] {
        assert!(names.contains(&n.to_string()), "{n} missing in {names:?}");
    }
    assert_eq!(d["healthy"], true);
    let calls = e.calls();
    assert!(
        calls.iter().all(|c| !c.iter().any(|a| a.contains("send"))),
        "doctor is read-only: {calls:?}"
    );
}

#[test]
fn pair_refuses_without_tailscale_running() {
    let e = Env::new();
    fs::write(
        e.root.join("fake/ts_status.json"),
        r#"{"BackendState":"Stopped","Self":null,"User":{}}"#,
    )
    .unwrap();
    let out = e.run(&["pair", "--timeout", "10"], None, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("tailscale is not running"));
}

#[test]
fn pair_refuses_on_a_tagged_host() {
    let e = Env::new();
    fs::write(
        e.root.join("fake/ts_status.json"),
        r#"{"BackendState":"Running","Self":{"UserID":1,"DNSName":"box.tail.ts.net.","Tags":["tag:server"]},"User":{"1":{"LoginName":"tagged-devices"}}}"#,
    )
    .unwrap();
    let out = e.run(&["pair", "--timeout", "10"], None, &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("tagged Tailscale node"), "{err}");
    assert!(
        !e.calls()
            .iter()
            .any(|c| c.get(1).map(String::as_str) == Some("ip")),
        "no listener is prepared"
    );
}

#[test]
fn integration_install_repoints_a_stale_hook_path() {
    let e = Env::new();
    let settings = e.root.join("settings.json");
    fs::write(
        &settings,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"'/old/openmosh-hook' emit --source claude"}]}]}}"#,
    )
    .unwrap();
    let s = settings.to_str().unwrap();
    let args = [
        "integration",
        "install",
        "claude",
        "--settings",
        s,
        "--json",
    ];
    let (code, v) = e.json(&args, None, &[]);
    assert_eq!(ok_data(code, &v)["changed"], true);
    let installed: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    for ev in ["Notification", "Stop"] {
        let hooks = installed["hooks"][ev].as_array().unwrap();
        assert_eq!(hooks.len(), 1, "{ev}");
        let cmd = hooks[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, format!("'{BIN}' emit --source claude"));
    }
    let (code, v) = e.json(&args, None, &[]);
    assert_eq!(ok_data(code, &v)["changed"], false);
}

/// Phone side of the QR pairing wire contract, written independently of the
/// hook's own modules (only the primitive crates are shared).
mod phone {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64U;
    use base64::Engine;
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    use hkdf::Hkdf;
    use sha2::Sha256;
    use x25519_dalek::{PublicKey, StaticSecret};

    pub fn keys(eph: &[u8; 32], hpk: &[u8; 32], tok: &[u8]) -> ([u8; 32], [u8; 32]) {
        let shared = StaticSecret::from(*eph).diffie_hellman(&PublicKey::from(*hpk));
        let hk = Hkdf::<Sha256>::new(Some(tok), shared.as_bytes());
        let (mut req, mut resp) = ([0u8; 32], [0u8; 32]);
        hk.expand(b"openmosh/pair/v1/req", &mut req).unwrap();
        hk.expand(b"openmosh/pair/v1/resp", &mut resp).unwrap();
        (req, resp)
    }

    pub fn request_line(eph: &[u8; 32], k_req: &[u8; 32], plaintext: &str) -> String {
        let nonce = [7u8; 24];
        let c = XChaCha20Poly1305::new(k_req.into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: b"openmosh/pair/v1",
                },
            )
            .unwrap();
        let e = PublicKey::from(&StaticSecret::from(*eph)).to_bytes();
        format!(
            "{{\"v\":1,\"e\":\"{}\",\"n\":\"{}\",\"c\":\"{}\"}}\n",
            B64U.encode(e),
            B64U.encode(nonce),
            B64U.encode(c)
        )
    }

    pub fn open_response(k_resp: &[u8; 32], line: &str) -> String {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let n = B64U.decode(v["n"].as_str().unwrap()).unwrap();
        let c = B64U.decode(v["c"].as_str().unwrap()).unwrap();
        let pt = XChaCha20Poly1305::new(k_resp.into())
            .decrypt(
                XNonce::from_slice(&n),
                Payload {
                    msg: &c,
                    aad: b"openmosh/pair/v1",
                },
            )
            .unwrap();
        String::from_utf8(pt).unwrap()
    }

    pub fn decode(s: &str) -> Vec<u8> {
        B64U.decode(s).unwrap()
    }
}

/// Full QR pairing over the real Tailscale interface of this machine:
/// binds the one-shot listener on the local 100.x address, plays the phone,
/// sends one bad and one good request. `tailscale ip -4` is the only real
/// (read-only) Tailscale call; status/whois are faked; HOME is temporary.
#[test]
#[ignore = "needs a local Tailscale IPv4; run with --ignored"]
fn qr_pairing_end_to_end_over_tailnet() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpStream;

    let real_ip = Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8(o.stdout)
                .ok()
                .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        });
    let Some(ip) = real_ip.filter(|ip| ip.starts_with("100.")) else {
        eprintln!("skipping: no local Tailscale IPv4");
        return;
    };
    let e = Env::new();
    fs::write(e.root.join("fake/ts_ip"), format!("{ip}\n")).unwrap();
    let mut child = e
        .cmd(&["pair", "--timeout", "30", "--uri", "--alias", "e2e"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut uri = String::new();
    let mut port = 0u16;
    let mut line = String::new();
    while out.read_line(&mut line).unwrap() > 0 {
        if line.trim().starts_with("openmosh://pair?") {
            uri = line.trim().to_string();
        }
        if line.starts_with("Listening on") {
            port = line
                .split(':')
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            break;
        }
        line.clear();
    }
    let offer: std::collections::HashMap<String, String> = url::Url::parse(&uri)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(offer["v"], "3");
    assert_eq!(offer["i"], ip.as_str());
    assert_eq!(offer["p"], port.to_string());
    assert_eq!(offer.len(), 5, "{uri}");
    let hpk: [u8; 32] = phone::decode(&offer["k"]).try_into().unwrap();
    let tok = phone::decode(&offer["t"]);
    let eph = [9u8; 32];
    let device_ssh =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH phone";
    let plaintext = format!(
        r#"{{"tok":"{}","name":"Pixel 8","ssh_pub":"{device_ssh}","ppk":"B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw","endpoint":"https://ntfy.sh/upE2E?up=1"}}"#,
        offer["t"]
    );
    let exchange = |req: &str| {
        let mut s = TcpStream::connect((ip.as_str(), port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut r = String::new();
        BufReader::new(s).read_line(&mut r).unwrap();
        r
    };

    // Wrong token: keys derived from another salt, AEAD fails, generic reply.
    let (bad_req, _) = phone::keys(&eph, &hpk, &[0u8; 16]);
    assert_eq!(
        exchange(&phone::request_line(&eph, &bad_req, &plaintext)),
        "{\"v\":1,\"err\":\"pair_rejected\"}\n"
    );

    let (k_req, k_resp) = phone::keys(&eph, &hpk, &tok);
    let reply = exchange(&phone::request_line(&eph, &k_req, &plaintext));
    let resp: Value = serde_json::from_str(&phone::open_response(&k_resp, reply.trim())).unwrap();
    assert_eq!(resp["alias"], "e2e");
    assert_eq!(
        resp["ssh_fp"],
        "SHA256:XmA4TG6/WNqReEVbojfXfv3nyHYvdvEr6WbFjs7VdLM"
    );
    assert!(resp["ssh_user"].as_str().is_some_and(|u| !u.is_empty()));
    assert!(resp["ssh_port"].as_u64().is_some());
    let device_id = resp["device_id"].as_str().unwrap().to_string();

    let status = child.wait().unwrap();
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut out, &mut rest).unwrap();
    assert!(status.success(), "{rest}");
    assert!(rest.contains("Paired Pixel 8"));
    let ak = fs::read_to_string(e.home().join(".ssh/authorized_keys")).unwrap();
    assert_eq!(
        ak,
        format!("from=\"100.64.0.0/10,fd7a:115c:a1e0::/48\",no-agent-forwarding,no-X11-forwarding,no-port-forwarding,no-user-rc ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH openmosh:{device_id}\n")
    );
    assert_eq!(mode(&e.home().join(".ssh/authorized_keys")), 0o600);
    assert_eq!(mode(&e.home().join(".ssh")), 0o700);
    // The listener is gone: single use.
    assert!(TcpStream::connect((ip.as_str(), port)).is_err());
}

/// Fake Herdr socket: acks the focus subscription, pushes one `tab_focused`
/// event, answers `session.snapshot`, and records every other request.
fn fake_herdr_socket(path: &Path, release: PathBuf) -> std::sync::mpsc::Receiver<Value> {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { break };
            let tx = tx.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                let mut w = conn.try_clone().unwrap();
                for line in BufReader::new(conn).lines() {
                    let Ok(line) = line else { break };
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let id = req["id"].as_str().unwrap_or_default().to_string();
                    let reply = match req["method"].as_str() {
                        Some("events.subscribe") => {
                            json!({"id": id, "result": {"type": "subscription_started"}})
                        }
                        Some("session.snapshot") => json!({"id": id, "result": {
                            "type": "session_snapshot",
                            "snapshot": {"focused_workspace_id": "w1", "focused_tab_id": "w1:t1"}
                        }}),
                        _ => json!({"id": id, "result": {"type": "ok"}}),
                    };
                    writeln!(w, "{reply}").unwrap();
                    match req["method"].as_str() {
                        Some("events.subscribe") => {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            let ev = json!({"event": "tab_focused", "data": {
                                "type": "tab_focused", "tab_id": "w1:t2", "workspace_id": "w1"}});
                            writeln!(w, "{ev}").unwrap();
                        }
                        Some("session.snapshot") => {}
                        _ => {
                            let _ = tx.send(req);
                            fs::write(&release, "").unwrap();
                        }
                    }
                }
            });
        }
    });
    rx
}

#[test]
fn attach_herdr_mirrors_focus_and_exits_with_the_child_code() {
    let env = Env::new();
    let sock = env.root.join("h.sock");
    let rx = fake_herdr_socket(&sock, env.root.join("fake/release"));
    let out = env.run(
        &["attach", "herdr"],
        None,
        &[("HERDR_SOCKET_PATH", sock.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let req = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
    assert_eq!(req["method"], "tab.focus");
    assert_eq!(req["params"], json!({"tab_id": "w1:t2"}));
    assert!(rx.try_recv().is_err(), "only one focus request");
    assert!(env.calls().iter().any(|c| c == &["herdr".to_string()]));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("unavailable"));
}

#[test]
fn attach_herdr_without_a_socket_still_runs_herdr() {
    let env = Env::new();
    fs::write(env.root.join("fake/release"), "").unwrap();
    let out = env.run(&["attach", "herdr", "--session", "work"], None, &[]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.matches("herdr socket unavailable").count(),
        1,
        "{stderr}"
    );
    assert!(env
        .calls()
        .iter()
        .any(|c| c == &["herdr", "session", "attach", "work"]));

    let bad = env.run(&["attach", "herdr", "--session", "../x"], None, &[]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(!env.calls().iter().any(|c| c.contains(&"../x".to_string())));
}
