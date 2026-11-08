use super::*;
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

fn tab(id: &str) -> Option<SocketEvent> {
    Some(SocketEvent::Focused(
        FocusTarget::new(FocusKind::Tab, id).unwrap(),
    ))
}

#[test]
fn parses_focus_events_and_events_lost() {
    let line = r#"{"event":"tab_focused","data":{"type":"tab_focused","tab_id":"w1:t2","workspace_id":"w1"}}"#;
    assert_eq!(parse_event_line(line).unwrap(), tab("w1:t2"));
    let dotted = r#"{"event":"pane.focused","data":{"pane_id":"w1:p3","workspace_id":"w1"}}"#;
    assert_eq!(
        parse_event_line(dotted).unwrap(),
        Some(SocketEvent::Focused(
            FocusTarget::new(FocusKind::Pane, "w1:p3").unwrap()
        ))
    );
    let ws = r#"{"event":"workspace_focused","data":{"workspace_id":"w2"}}"#;
    assert!(matches!(
        parse_event_line(ws).unwrap(),
        Some(SocketEvent::Focused(FocusTarget {
            kind: FocusKind::Workspace,
            ..
        }))
    ));
    let lost = r#"{"id":"s","error":{"code":"events_lost","message":"x"}}"#;
    assert_eq!(
        parse_event_line(lost).unwrap(),
        Some(SocketEvent::EventsLost)
    );
}

#[test]
fn ignores_other_events_and_refuses_odd_ids() {
    let other = r#"{"event":"pane_created","data":{"pane_id":"w1:p3"}}"#;
    assert_eq!(parse_event_line(other).unwrap(), None);
    let bad_id = r#"{"event":"tab_focused","data":{"tab_id":"w1 t2; rm"}}"#;
    assert_eq!(parse_event_line(bad_id).unwrap(), None);
    let ack = r#"{"id":"s","result":{"type":"subscription_started"}}"#;
    assert_eq!(parse_event_line(ack).unwrap(), None);
    assert!(parse_event_line("{not json").is_err());
    let err = r#"{"id":"s","error":{"code":"pane_not_found","message":"x"}}"#;
    assert!(parse_event_line(err).is_err());
}

#[test]
fn snapshot_reads_focused_ids() {
    let v = json!({"type":"session_snapshot","snapshot":{
        "focused_workspace_id":"w1","focused_tab_id":"w1:t2","focused_pane_id":null,
        "workspaces":[],"tabs":[],"panes":[],"layouts":[],"agents":[]}});
    assert_eq!(
        parse_snapshot(&v),
        FocusSnapshot {
            workspace: Some("w1".into()),
            tab: Some("w1:t2".into()),
            pane: None
        }
    );
}

#[test]
fn socket_path_follows_herdr_resolution_order() {
    let cfg = Path::new("/h/.config");
    let none = |_: &str| None;
    assert_eq!(
        socket_path(None, none, cfg),
        PathBuf::from("/h/.config/herdr/herdr.sock")
    );
    assert_eq!(
        socket_path(Some("work"), none, cfg),
        PathBuf::from("/h/.config/herdr/sessions/work/herdr.sock")
    );
    let env = |k: &str| match k {
        "HERDR_SOCKET_PATH" => Some("/run/x.sock".to_string()),
        "HERDR_SESSION" => Some("side".to_string()),
        _ => None,
    };
    assert_eq!(
        socket_path(Some("work"), env, cfg).file_name().unwrap(),
        "herdr.sock"
    );
    assert_eq!(socket_path(None, env, cfg), PathBuf::from("/run/x.sock"));
    let env2 = |k: &str| match k {
        "HERDR_SOCKET_PATH" => Some("relative.sock".to_string()),
        "HERDR_SESSION" => Some("side".to_string()),
        _ => None,
    };
    assert_eq!(
        socket_path(None, env2, cfg),
        PathBuf::from("/h/.config/herdr/sessions/side/herdr.sock")
    );
    let bad = |k: &str| (k == "HERDR_SESSION").then(|| "../../etc".to_string());
    assert_eq!(
        socket_path(None, bad, cfg),
        PathBuf::from("/h/.config/herdr/herdr.sock")
    );
}

fn read_req(r: &mut BufReader<UnixStream>) -> Value {
    let mut l = String::new();
    r.read_line(&mut l).unwrap();
    serde_json::from_str(&l).unwrap()
}

#[test]
fn talks_to_a_fake_herdr_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, rx) = mpsc::channel::<Value>();
    let server = std::thread::spawn(move || {
        let (sub, _) = listener.accept().unwrap();
        let mut sub_r = BufReader::new(sub.try_clone().unwrap());
        let mut sub_w = sub;
        let req = read_req(&mut sub_r);
        tx.send(req.clone()).unwrap();
        let id = req["id"].as_str().unwrap();
        writeln!(
            sub_w,
            r#"{{"id":"{id}","result":{{"type":"subscription_started"}}}}"#
        )
        .unwrap();
        let (cmd, _) = listener.accept().unwrap();
        let mut cmd_r = BufReader::new(cmd.try_clone().unwrap());
        let mut cmd_w = cmd;
        for _ in 0..2 {
            let req = read_req(&mut cmd_r);
            tx.send(req.clone()).unwrap();
            let id = req["id"].as_str().unwrap();
            let result = if req["method"] == "session.snapshot" {
                json!({"type":"session_snapshot","snapshot":{"focused_tab_id":"w1:t1"}})
            } else {
                json!({"type":"ok"})
            };
            // An unrelated line first: replies are matched by id.
            writeln!(cmd_w, r#"{{"id":"other","result":{{"type":"ok"}}}}"#).unwrap();
            writeln!(cmd_w, "{}", json!({"id": id, "result": result})).unwrap();
        }
        // An event split across writes, preceded by an ignored one.
        sub_w
            .write_all(b"{\"event\":\"pane_created\",\"data\":{}}\n{\"event\":\"tab_fo")
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        sub_w
            .write_all(b"cused\",\"data\":{\"tab_id\":\"w1:t2\",\"workspace_id\":\"w1\"}}\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });

    let mut s = HerdrSocket::new(path);
    s.connect().unwrap();
    let sub = rx.recv().unwrap();
    assert_eq!(sub["method"], "events.subscribe");
    assert_eq!(
        sub["params"]["subscriptions"],
        json!([{"type":"workspace.focused"},{"type":"tab.focused"},{"type":"pane.focused"}])
    );
    assert_eq!(s.snapshot().unwrap().tab.as_deref(), Some("w1:t1"));
    assert_eq!(rx.recv().unwrap()["method"], "session.snapshot");
    s.focus(&FocusTarget::new(FocusKind::Tab, "w1:t9").unwrap())
        .unwrap();
    let focus = rx.recv().unwrap();
    assert_eq!(focus["method"], "tab.focus");
    assert_eq!(focus["params"], json!({"tab_id":"w1:t9"}));
    let mut got = None;
    for _ in 0..20 {
        if let Some(ev) = s.next_event(Duration::from_millis(50)).unwrap() {
            got = Some(ev);
            break;
        }
    }
    assert_eq!(got, tab("w1:t2"));
    server.join().unwrap();
    // The server is gone: the subscription reports a closed socket.
    assert_eq!(
        s.next_event(Duration::from_millis(50)),
        Err(MirrorError::Closed)
    );
}

#[test]
fn missing_socket_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = HerdrSocket::new(dir.path().join("absent.sock"));
    assert_eq!(s.connect(), Err(MirrorError::Unavailable));
}

#[test]
fn launcher_returns_the_child_exit_code_and_signal() {
    let mut c = TtyLauncher
        .spawn("sh", &["-c".into(), "exit 3".into()])
        .unwrap();
    assert_eq!(c.wait().unwrap(), 3);
    let mut k = TtyLauncher
        .spawn("sh", &["-c".into(), "kill -9 $$".into()])
        .unwrap();
    assert_eq!(k.wait().unwrap(), 137);
    assert!(matches!(
        TtyLauncher.spawn("openmosh-definitely-missing-binary", &[]),
        Err(MirrorError::Launch(_))
    ));
}
