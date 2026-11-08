//! Shared harness for the CLI tests: the real binary against a temporary
//! HOME/XDG tree and fake `herdr`, `tmux`, `tailscale`, `systemctl` and
//! `launchctl` scripts that only log their argv.
#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const BIN: &str = env!("CARGO_BIN_EXE_openmosh-hook");
pub const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJYwG729z8R85TUE6pHTEWVG3RxgVSZik0RW18NtXepH root@host";
pub const AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"blocked","cwd":"/w","pane_id":"w7:pA","state_change_seq":21,"terminal_title_stripped":"Fix tests","workspace_id":"w7"}],"type":"agent_list"}}"#;

pub const FAKE: &str = r#"#!/bin/sh
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
  "tmux display-message -p"|"tmux -S "*)
    case "$*" in
      *display-message*) [ -f "$FAKE_DIR/tmux_pid" ] || { echo "can't find pane" >&2; exit 1; }; cat "$FAKE_DIR/tmux_pid" ;;
    esac ;;
  "tailscale status --json") cat "$FAKE_DIR/ts_status.json" ;;
  "tailscale ip -4") cat "$FAKE_DIR/ts_ip" ;;
  "tailscale whois --json") echo '{"Node":{},"UserProfile":{"LoginName":"me@example.com"}}' ;;
  "systemctl --user is-active") echo inactive; exit 3 ;;
esac
exit 0
"#;

pub struct Env {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    /// `PATH` given to the binary.
    pub path: String,
}

impl Env {
    pub fn new() -> Self {
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
        fs::write(fake.join("tmux_pid"), "4242\n").unwrap();
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
        let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
        Self {
            _dir: dir,
            root,
            path,
        }
    }

    /// Like `new`, but `PATH` holds only the fakes and the few tools they
    /// need, so agents installed on the real machine are never detected.
    pub fn isolated() -> Self {
        let mut e = Self::new();
        let tools = e.root.join("tools");
        fs::create_dir_all(&tools).unwrap();
        for t in ["basename", "cat", "sleep"] {
            let real = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(t))
                .find(|p| p.exists())
                .unwrap_or_else(|| panic!("{t} not found"));
            std::os::unix::fs::symlink(real, tools.join(t)).unwrap();
        }
        e.path = format!("{}:{}", e.root.join("bin").display(), tools.display());
        e
    }

    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn state(&self) -> PathBuf {
        self.root.join("state/openmosh")
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("PATH", &self.path)
            .env("USER", "tester")
            .env("FAKE_LOG", self.root.join("calls.log"))
            .env("FAKE_DIR", self.root.join("fake"));
        c
    }

    pub fn run(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Output {
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
    pub fn json(
        &self,
        args: &[&str],
        stdin: Option<&str>,
        extra_env: &[(&str, &str)],
    ) -> (i32, Value) {
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
    pub fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .split('\u{1e}')
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\u{1f}').map(str::to_string).collect())
            .collect()
    }
}

pub fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

pub fn ok_data(code: i32, v: &Value) -> &Value {
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    &v["data"]
}

pub fn err_code(code: i32, v: &Value) -> String {
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"]["message"].is_string());
    v["error"]["code"].as_str().unwrap().to_string()
}
