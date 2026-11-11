//! End-to-end tests of `herdr snapshot|read|action`: the real binary against
//! a fake Herdr socket in a temporary XDG tree (never a real Herdr session)
//! and the fake `herdr` script for the CLI send-text path.

mod common;

use common::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};

const SNAPSHOT: &str = r#"{"type":"session_snapshot","snapshot":{"version":"0.9.0","protocol":22,
 "workspaces":[{"workspace_id":"w1","number":1,"label":"api","focused":true,"pane_count":1,"tab_count":1,"active_tab_id":"w1:t1","agent_status":"blocked"}],
 "tabs":[{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"1","focused":true,"pane_count":1,"agent_status":"blocked"}],
 "panes":[{"pane_id":"w1:p1","terminal_id":"t","workspace_id":"w1","tab_id":"w1:t1","focused":true,"agent":"claude","agent_status":"blocked","revision":3}],
 "layouts":[],"agents":[]}}"#;

/// Serves Herdr requests on `<XDG_CONFIG_HOME>/herdr/herdr.sock` (or the
/// named session's socket) and records them.
struct FakeHerdr {
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FakeHerdr {
    fn start(e: &Env, session: Option<&str>) -> Self {
        let dir = match session {
            Some(s) => e.root.join("config/herdr/sessions").join(s),
            None => e.root.join("config/herdr"),
        };
        std::fs::create_dir_all(&dir).unwrap();
        let listener = UnixListener::bind(dir.join("herdr.sock")).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(c) = conn else { return };
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut w = c;
                let mut l = String::new();
                if r.read_line(&mut l).unwrap_or(0) == 0 {
                    continue;
                }
                let req: Value = serde_json::from_str(&l).unwrap();
                log.lock().unwrap().push(req.clone());
                let result: Value = match req["method"].as_str().unwrap() {
                    "session.snapshot" => serde_json::from_str(SNAPSHOT).unwrap(),
                    "pane.read" => json!({"type":"pane_read","read":{"pane_id":"w1:p1",
                        "text":"\u{1b}[1mAllow Bash?\u{1b}[0m\n❯ 1. Yes\n  2. No\n"}}),
                    "workspace.create" => json!({"type":"workspace_created",
                        "workspace":{"workspace_id":"w2"},"tab":{"tab_id":"w2:t1"}}),
                    _ => json!({"type":"ok"}),
                };
                let _ = writeln!(w, "{}", json!({"id": req["id"], "result": result}));
            }
        });
        Self { requests }
    }

    fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r["method"].as_str().unwrap().to_string())
            .collect()
    }

    fn last(&self) -> Value {
        self.requests.lock().unwrap().last().unwrap().clone()
    }
}

#[test]
fn snapshot_then_unchanged_with_the_same_rev() {
    let e = Env::new();
    let _h = FakeHerdr::start(&e, None);
    let (code, v) = e.json(&["herdr", "snapshot", "--json"], None, &[]);
    let d = ok_data(code, &v).clone();
    let rev = d["rev"].as_str().unwrap().to_string();
    assert_eq!(rev.len(), 16);
    assert_eq!(d["unchanged"], false);
    assert_eq!(
        d["workspaces"],
        json!([{"id":"w1","label":"api","focused":true}])
    );
    assert_eq!(
        d["panes"],
        json!([{"id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","agent":"claude","agent_status":"blocked","focused":true}])
    );
    assert_eq!(d["tabs"][0]["agent_status"], "blocked");

    let start = std::time::Instant::now();
    let (code, v) = e.json(
        &[
            "herdr",
            "snapshot",
            "--since",
            &rev,
            "--wait-ms",
            "600",
            "--json",
        ],
        None,
        &[],
    );
    assert_eq!(ok_data(code, &v), &json!({"rev": rev, "unchanged": true}));
    assert!(start.elapsed().as_millis() >= 550, "waited for a change");
}

#[test]
fn read_returns_the_pane_screen() {
    let e = Env::new();
    let h = FakeHerdr::start(&e, Some("work"));
    let (code, v) = e.json(
        &[
            "herdr",
            "read",
            "--pane",
            "w1:p1",
            "--session",
            "work",
            "--lines",
            "2",
            "--json",
        ],
        None,
        &[],
    );
    let d = ok_data(code, &v);
    assert_eq!(d["agent"], "claude");
    assert_eq!(d["agent_status"], "blocked");
    assert_eq!(d["lines"], json!(["❯ 1. Yes", "  2. No"]));
    assert_eq!(h.last()["params"]["lines"], 2);

    for (args, why) in [
        (
            vec!["--pane", "w1:p9", "--session", "work"],
            "pane not in the snapshot",
        ),
        (
            vec!["--pane", "w1:p1; rm", "--session", "work"],
            "bad pane id",
        ),
        (vec!["--pane", "w1:p1", "--lines", "401"], "too many lines"),
        (vec!["--pane", "w1:p1", "--lines", "0"], "zero lines"),
        (vec!["--pane", "w1:p1", "--session", "../x"], "bad session"),
    ] {
        let mut argv = vec!["herdr", "read"];
        argv.extend(args);
        argv.push("--json");
        let (code, v) = e.json(&argv, None, &[]);
        assert_eq!(err_code(code, &v), "invalid_request", "{why}");
    }
}

#[test]
fn actions_run_only_from_the_allowlist() {
    let e = Env::new();
    let h = FakeHerdr::start(&e, None);
    let (code, v) = e.json(
        &["herdr", "action", "--json"],
        Some(r#"{"op":"workspace.create","label":"new"}"#),
        &[],
    );
    assert_eq!(
        ok_data(code, &v),
        &json!({"op":"workspace.create","id":"w2"})
    );
    assert_eq!(h.last()["params"], json!({"label":"new","focus":false}));

    let (code, v) = e.json(
        &["herdr", "action", "--json"],
        Some(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["down","enter"]}"#),
        &[],
    );
    assert_eq!(ok_data(code, &v), &json!({"op":"pane.keys"}));
    assert_eq!(h.last()["method"], "pane.send_keys");

    let (code, v) = e.json(
        &["herdr", "action", "--json"],
        Some(r#"{"op":"pane.text","pane_id":"w1:p1","text":"-y $(id)","submit":true}"#),
        &[],
    );
    ok_data(code, &v);
    // Over the socket, not the `herdr` binary (often off PATH over SSH).
    assert!(e.calls().is_empty(), "{:?}", e.calls());
    let methods = h.methods();
    assert_eq!(
        &methods[methods.len() - 2..],
        ["pane.send_text", "pane.send_keys"]
    );
    assert_eq!(
        h.last()["params"],
        json!({"pane_id":"w1:p1","keys":["enter"]})
    );

    let before = h.methods().len();
    for body in [
        r#"{"op":"server.stop"}"#,
        r#"{"op":"workspace.close","id":"w1","close_group":true}"#,
        r#"{"op":"workspace.close","id":"w9"}"#,
        r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["C-c"]}"#,
        r#"{"op":"agent.start","pane_id":"w1:p1","agent":"sh"}"#,
    ] {
        let (code, v) = e.json(&["herdr", "action", "--json"], Some(body), &[]);
        assert_eq!(err_code(code, &v), "invalid_request", "{body}");
    }
    let after: Vec<_> = h.methods().split_off(before);
    assert!(
        after.iter().all(|m| m == "session.snapshot"),
        "rejected actions only read the snapshot: {after:?}"
    );
}

#[test]
fn without_a_herdr_socket_the_runtime_is_unavailable() {
    let e = Env::new();
    let (code, v) = e.json(&["herdr", "snapshot", "--json"], None, &[]);
    assert_eq!(err_code(code, &v), "runtime_unavailable");
}
