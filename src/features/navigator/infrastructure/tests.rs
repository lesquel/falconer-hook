use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

/// A `session.snapshot` reply as herdr 0.9.0 prints it, plus odd entries.
const SNAPSHOT: &str = r#"{"type":"session_snapshot","snapshot":{"version":"0.9.0","protocol":22,
 "focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1",
 "workspaces":[{"workspace_id":"w1","number":1,"label":"alpha\u001b[31m\nx","focused":true,"pane_count":2,"tab_count":2,"active_tab_id":"w1:t1","agent_status":"unknown"},
   {"workspace_id":"bad id","label":"x"}],
 "tabs":[{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"1","focused":true,"pane_count":1,"agent_status":"blocked"},
   {"tab_id":"w1:t2","workspace_id":"w1","number":2,"label":"beta","focused":false,"pane_count":1,"agent_status":"sleeping"}],
 "panes":[{"pane_id":"w1:p1","terminal_id":"term_1","workspace_id":"w1","tab_id":"w1:t1","focused":true,"cwd":"/home/u","agent":"claude","agent_status":"blocked","revision":7},
   {"pane_id":"w1:p2","terminal_id":"term_2","workspace_id":"w1","tab_id":"w1:t2","focused":false,"agent_status":"unknown","revision":0}],
 "layouts":[],"agents":[]}}"#;

#[test]
fn maps_the_real_snapshot_shape() {
    let snap = parse_nav_snapshot(&serde_json::from_str(SNAPSHOT).unwrap());
    let v = serde_json::to_value(&snap).unwrap();
    assert_eq!(
        v,
        json!({
            "workspaces":[{"id":"w1","label":"alpha[31m x","focused":true}],
            "tabs":[
                {"id":"w1:t1","workspace_id":"w1","label":"1","number":1,"agent_status":"blocked","pane_count":1,"focused":true},
                {"id":"w1:t2","workspace_id":"w1","label":"beta","number":2,"agent_status":"unknown","pane_count":1,"focused":false}],
            "panes":[
                {"id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","agent":"claude","agent_status":"blocked","focused":true},
                {"id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","agent":null,"agent_status":"unknown","focused":false}]
        })
    );
}

#[test]
fn pane_output_does_not_change_the_snapshot() {
    use crate::features::navigator::domain::rev_of;
    let a = parse_nav_snapshot(&serde_json::from_str(SNAPSHOT).unwrap());
    let b = parse_nav_snapshot(
        &serde_json::from_str(&SNAPSHOT.replace("\"revision\":7", "\"revision\":8")).unwrap(),
    );
    assert_eq!(
        rev_of(&a),
        rev_of(&b),
        "rev hashes only the returned fields"
    );
}

fn op(body: &str) -> Op {
    Op::parse(body).unwrap()
}

#[test]
fn every_socket_action_maps_to_its_herdr_method() {
    let cases = [
        (
            r#"{"op":"workspace.create","label":"api"}"#,
            "workspace.create",
            json!({"label":"api","focus":false}),
        ),
        (
            r#"{"op":"workspace.create"}"#,
            "workspace.create",
            json!({"label":null,"focus":false}),
        ),
        (
            r#"{"op":"workspace.rename","id":"w1","label":"x"}"#,
            "workspace.rename",
            json!({"workspace_id":"w1","label":"x"}),
        ),
        (
            r#"{"op":"workspace.close","id":"w1"}"#,
            "workspace.close",
            json!({"workspace_id":"w1"}),
        ),
        (
            r#"{"op":"workspace.focus","id":"w1"}"#,
            "workspace.focus",
            json!({"workspace_id":"w1"}),
        ),
        (
            r#"{"op":"tab.create","workspace_id":"w1","label":"b"}"#,
            "tab.create",
            json!({"workspace_id":"w1","label":"b","focus":false}),
        ),
        (
            r#"{"op":"tab.rename","id":"w1:t1","label":"x"}"#,
            "tab.rename",
            json!({"tab_id":"w1:t1","label":"x"}),
        ),
        (
            r#"{"op":"tab.close","id":"w1:t1"}"#,
            "tab.close",
            json!({"tab_id":"w1:t1"}),
        ),
        (
            r#"{"op":"tab.focus","id":"w1:t1"}"#,
            "tab.focus",
            json!({"tab_id":"w1:t1"}),
        ),
        (
            r#"{"op":"pane.focus","id":"w1:p1"}"#,
            "pane.focus",
            json!({"pane_id":"w1:p1"}),
        ),
        (
            r#"{"op":"agent.start","pane_id":"w1:p1","agent":"codex"}"#,
            "agent.start",
            json!({"name":"codex","kind":"codex","pane_id":"w1:p1","timeout_ms":20000}),
        ),
        (
            r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["down","enter"]}"#,
            "pane.send_keys",
            json!({"pane_id":"w1:p1","keys":["down","enter"]}),
        ),
    ];
    for (body, method, params) in cases {
        let (m, p, wait) = socket_call(&op(body)).unwrap();
        assert_eq!((m, &p), (method, &params), "{body}");
        assert!(wait <= Duration::from_secs(25));
    }
    assert!(socket_call(&op(r#"{"op":"pane.text","pane_id":"w1:p1","text":"x"}"#)).is_none());
}

/// A fake Herdr socket: one request per connection, replies from `reply`.
fn serve(
    n: usize,
    reply: impl Fn(&Value) -> Value + Send + 'static,
) -> (
    tempfile::TempDir,
    PathBuf,
    mpsc::Receiver<Value>,
    std::thread::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, rx) = mpsc::channel();
    let h = std::thread::spawn(move || {
        for _ in 0..n {
            let (c, _) = listener.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut w = c;
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            let req: Value = serde_json::from_str(&l).unwrap();
            tx.send(req.clone()).unwrap();
            let mut out = reply(&req);
            out["id"] = req["id"].clone();
            writeln!(w, r#"{{"id":"other","result":{{"type":"ok"}}}}"#).unwrap();
            writeln!(w, "{out}").unwrap();
        }
    });
    (dir, path, rx, h)
}

#[test]
fn talks_to_a_fake_herdr_socket() {
    let (_d, path, rx, h) = serve(4, |req| match req["method"].as_str().unwrap() {
        "session.snapshot" => json!({"result": serde_json::from_str::<Value>(SNAPSHOT).unwrap()}),
        "pane.read" => {
            json!({"result":{"type":"pane_read","read":{"pane_id":"w1:p1","text":"a\nb\n"}}})
        }
        "workspace.create" => {
            json!({"result":{"type":"workspace_created","workspace":{"workspace_id":"w3"},"tab":{"tab_id":"w3:t1"}}})
        }
        _ => json!({"error":{"code":"Pane_Not_Found; rm","message":"x"}}),
    });
    let mut nav = SocketNavigator::new(path, None);
    assert_eq!(nav.snapshot().unwrap().panes.len(), 2);
    assert_eq!(rx.recv().unwrap()["method"], "session.snapshot");
    assert_eq!(nav.read_pane("w1:p1", 50).unwrap(), "a\nb\n");
    assert_eq!(
        rx.recv().unwrap()["params"],
        json!({"pane_id":"w1:p1","source":"recent","lines":50,"format":"text","strip_ansi":true})
    );
    assert_eq!(
        nav.apply(&op(r#"{"op":"workspace.create","label":"n"}"#))
            .unwrap(),
        Some("w3".into())
    );
    rx.recv().unwrap();
    let err = nav
        .apply(&op(r#"{"op":"tab.close","id":"w1:t1"}"#))
        .unwrap_err();
    assert_eq!(err, NavError::Refused("pane_not_foundrm".into()));
    assert_eq!(rx.recv().unwrap()["method"], "tab.close");
    h.join().unwrap();
    assert_eq!(nav.snapshot(), Err(NavError::Unavailable), "server gone");
}

#[test]
fn a_socket_of_another_user_is_not_used() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let _l = UnixListener::bind(&path).unwrap();
    let me = std::fs::metadata(&path).unwrap().uid();
    let mut nav = SocketNavigator::new(path, Some(me.wrapping_add(1)));
    assert_eq!(nav.snapshot(), Err(NavError::Unavailable));
}
