use super::*;
use crate::features::integration::domain::CATALOG;

fn snap() -> NavSnapshot {
    NavSnapshot {
        workspaces: vec![NavWorkspace {
            id: "w1".into(),
            label: "api".into(),
            focused: true,
        }],
        tabs: vec![NavTab {
            id: "w1:t1".into(),
            workspace_id: "w1".into(),
            label: "1".into(),
            number: 1,
            agent_status: AgentStatus::Working,
            pane_count: 1,
            focused: true,
        }],
        panes: vec![NavPane {
            id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            agent: Some("claude".into()),
            agent_status: AgentStatus::Working,
            focused: true,
        }],
    }
}

fn check(body: &str) -> Result<Op, NavError> {
    let op = Op::parse(body)?;
    validate(&op, &snap())?;
    Ok(op)
}

fn refused(body: &str) -> String {
    match check(body) {
        Err(e) => {
            assert_eq!(e.code(), "invalid_request", "{body}");
            e.to_string()
        }
        Ok(op) => panic!("accepted {body}: {op:?}"),
    }
}

#[test]
fn parses_every_op_of_the_contract() {
    let ok = [
        (r#"{"op":"workspace.create"}"#, "workspace.create"),
        (
            r#"{"op":"workspace.create","label":"api"}"#,
            "workspace.create",
        ),
        (
            r#"{"op":"workspace.rename","id":"w1","label":"x"}"#,
            "workspace.rename",
        ),
        (r#"{"op":"workspace.close","id":"w1"}"#, "workspace.close"),
        (r#"{"op":"workspace.focus","id":"w1"}"#, "workspace.focus"),
        (r#"{"op":"tab.create","workspace_id":"w1"}"#, "tab.create"),
        (
            r#"{"op":"tab.rename","id":"w1:t1","label":"x"}"#,
            "tab.rename",
        ),
        (r#"{"op":"tab.close","id":"w1:t1"}"#, "tab.close"),
        (r#"{"op":"tab.focus","id":"w1:t1"}"#, "tab.focus"),
        (r#"{"op":"pane.focus","id":"w1:p1"}"#, "pane.focus"),
        (
            r#"{"op":"agent.start","pane_id":"w1:p1","agent":"codex"}"#,
            "agent.start",
        ),
        (
            r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["down","9","enter"]}"#,
            "pane.keys",
        ),
        (
            r#"{"op":"pane.text","pane_id":"w1:p1","text":"hi","submit":true}"#,
            "pane.text",
        ),
        (
            r#"{"op":"pane.text","pane_id":"w1:p1","text":"hi"}"#,
            "pane.text",
        ),
    ];
    for (body, name) in ok {
        assert_eq!(check(body).unwrap().name(), name, "{body}");
    }
    assert_eq!(
        Op::parse(r#"{"op":"pane.text","pane_id":"w1:p1","text":"hi"}"#).unwrap(),
        Op::PaneText(Text {
            pane_id: "w1:p1".into(),
            text: "hi".into(),
            submit: false
        })
    );
}

#[test]
fn refuses_unknown_ops_and_fields() {
    refused(r#"{"op":"server.stop"}"#);
    refused(r#"{"op":"pane.run","pane_id":"w1:p1","command":"rm -rf ~"}"#);
    refused(r#"{"id":"w1"}"#);
    refused(r#"{"op":"workspace.close","id":"w1","close_group":true}"#);
    refused(r#"{"op":"workspace.create","label":"a","cwd":"/etc"}"#);
    refused(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["enter"],"session":"x"}"#);
    refused(r#"{"op":"tab.rename","id":"w1:t1"}"#);
    refused("not json");
}

#[test]
fn refuses_bad_ids_and_ids_missing_from_the_snapshot() {
    refused(r#"{"op":"workspace.focus","id":"w1; rm -rf"}"#);
    refused(r#"{"op":"workspace.focus","id":""}"#);
    refused(r#"{"op":"pane.focus","id":"-w1:p1"}"#);
    let missing = refused(r#"{"op":"workspace.close","id":"w9"}"#);
    assert!(missing.contains("not found"), "{missing}");
    refused(r#"{"op":"tab.close","id":"w1:t9"}"#);
    refused(r#"{"op":"tab.focus","id":"w1"}"#);
    refused(r#"{"op":"tab.create","workspace_id":"w2"}"#);
    refused(r#"{"op":"pane.keys","pane_id":"w1:p9","keys":["enter"]}"#);
    refused(r#"{"op":"agent.start","pane_id":"w2:p1","agent":"claude"}"#);
}

#[test]
fn refuses_bad_labels() {
    let long = "x".repeat(65);
    refused(&format!(
        r#"{{"op":"workspace.rename","id":"w1","label":"{long}"}}"#
    ));
    assert!(check(&format!(
        r#"{{"op":"workspace.rename","id":"w1","label":"{}"}}"#,
        "é".repeat(64)
    ))
    .is_ok());
    refused(r#"{"op":"workspace.rename","id":"w1","label":""}"#);
    refused(r#"{"op":"workspace.rename","id":"w1","label":"   "}"#);
    refused(r#"{"op":"tab.rename","id":"w1:t1","label":"a\u001b[31mb"}"#);
    refused(r#"{"op":"tab.create","workspace_id":"w1","label":"a\nb"}"#);
    refused("{\"op\":\"workspace.create\",\"label\":\"txt\\u202eexe\"}");
}

#[test]
fn refuses_keys_outside_the_allowlist() {
    let bad = refused(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["enter","C-c"]}"#);
    assert!(bad.contains("Cc"), "{bad}");
    refused(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["0"]}"#);
    refused(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":["Enter"]}"#);
    refused(r#"{"op":"pane.keys","pane_id":"w1:p1","keys":[]}"#);
    let nine = format!(
        r#"{{"op":"pane.keys","pane_id":"w1:p1","keys":[{}]}}"#,
        [r#""y""#; 9].join(",")
    );
    refused(&nine);
    let eight = nine.replacen(r#""y","#, "", 1);
    assert!(check(&eight).is_ok());
    for k in ALLOWED_KEYS {
        assert!(check(&format!(
            r#"{{"op":"pane.keys","pane_id":"w1:p1","keys":["{k}"]}}"#
        ))
        .is_ok());
    }
}

#[test]
fn refuses_long_text_and_unknown_agents() {
    let ok = format!(
        r#"{{"op":"pane.text","pane_id":"w1:p1","text":"{}"}}"#,
        "é".repeat(4000)
    );
    assert!(check(&ok).is_ok());
    let long = format!(
        r#"{{"op":"pane.text","pane_id":"w1:p1","text":"{}"}}"#,
        "a".repeat(4001)
    );
    refused(&long);
    refused(r#"{"op":"agent.start","pane_id":"w1:p1","agent":"aider"}"#);
    refused(r#"{"op":"agent.start","pane_id":"w1:p1","agent":"bash"}"#);
    refused(r#"{"op":"agent.start","pane_id":"w1:p1","agent":"claude --dangerously"}"#);
}

#[test]
fn startable_agents_come_from_the_hook_catalog() {
    for spec in CATALOG {
        let id = spec.id.as_str();
        assert_eq!(STARTABLE_AGENTS.contains(&id), id != "aider", "{id}");
    }
}

#[test]
fn rev_is_stable_and_follows_content() {
    let a = snap();
    let r = rev_of(&a);
    assert_eq!(r.len(), 16);
    assert!(r.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(rev_of(&a.clone()), r);
    let mut b = a.clone();
    b.panes[0].agent_status = AgentStatus::Blocked;
    assert_ne!(rev_of(&b), r);
}

#[test]
fn polled_serializes_the_contract_shape() {
    let full = Polled {
        rev: "abc".into(),
        unchanged: false,
        body: Some(snap()),
    };
    let v = serde_json::to_value(&full).unwrap();
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["rev", "unchanged", "workspaces", "tabs", "panes"]);
    assert_eq!(
        v["tabs"][0],
        serde_json::json!({"id":"w1:t1","workspace_id":"w1","label":"1","number":1,
            "agent_status":"working","pane_count":1,"focused":true})
    );
    assert_eq!(
        v["panes"][0],
        serde_json::json!({"id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1",
            "agent":"claude","agent_status":"working","focused":true})
    );
    let same: Polled<NavSnapshot> = Polled {
        rev: "abc".into(),
        unchanged: true,
        body: None,
    };
    assert_eq!(
        serde_json::to_string(&same).unwrap(),
        r#"{"rev":"abc","unchanged":true}"#
    );
}

#[test]
fn screen_lines_strip_escapes_and_keep_the_last_lines() {
    let text = "\u{1b}[31mRED\u{1b}[0m ok  \r\n\u{1b}]0;title\u{7}tab\there\u{1b}(B\n\u{9b}1mbold\u{1b}]8;;http://x\u{1b}\\link\n\u{202e}evil\u{7}\n\n\n";
    assert_eq!(
        screen_lines(text, 400),
        vec!["RED ok", "tab here", "boldlink", "evil"]
    );
    let many: String = (1..=500).map(|i| format!("l{i}\n")).collect();
    let got = screen_lines(&many, 400);
    assert_eq!(got.len(), 400);
    assert_eq!((got[0].as_str(), got[399].as_str()), ("l101", "l500"));
    assert_eq!(screen_lines(&many, 2), vec!["l499", "l500"]);
    assert!(screen_lines("", 10).is_empty());
}
