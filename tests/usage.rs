//! `usage` through the CLI against a temporary HOME with fixture logs.

mod common;

use common::*;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

fn utc(args: &[&str]) -> String {
    let out = Command::new("date")
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn now_rfc3339() -> String {
    utc(&["+%Y-%m-%dT%H:%M:%SZ"])
}

fn claude_line(id: &str, ts: &str, cwd: &str, u: [u64; 4]) -> String {
    format!(
        r#"{{"type":"assistant","cwd":"{cwd}","requestId":"req_{id}","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus","content":[{{"type":"text","text":"do not print me"}}],"usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":{},"cache_read_input_tokens":{}}}}}}}"#,
        u[0], u[1], u[2], u[3]
    )
}

fn write_fixtures(env: &Env) {
    let ts = now_rfc3339();
    let home = env.home();
    let claude = home.join(".claude/projects/-w-OpenMosh");
    fs::create_dir_all(&claude).unwrap();
    let line = claude_line("m1", &ts, "/w/OpenMosh", [10, 20, 30, 40]);
    let body = [
        line.clone(),
        // The same streamed message again: counted once.
        line,
        "{broken \"usage\"".to_string(),
        claude_line("m2", "2020-01-01T00:00:00Z", "/w/Old", [1000, 0, 0, 0]),
        claude_line("m3", &ts, "/w/Other", [1, 1, 1, 1]),
    ]
    .join("\n");
    fs::write(claude.join("s.jsonl"), body).unwrap();

    let codex = home.join(".codex/sessions/2026/11/08");
    fs::create_dir_all(&codex).unwrap();
    let resets_at = utc(&["+%s"]).parse::<u64>().unwrap() + 3600;
    let body = [
        format!(r#"{{"timestamp":"{ts}","type":"session_meta","payload":{{"id":"s","cwd":"/w/App"}}}}"#),
        format!(r#"{{"timestamp":"{ts}","type":"turn_context","payload":{{"cwd":"/w/App","model":"gpt-5"}}}}"#),
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":1050}},"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":1050}}}},"rate_limits":{{"primary":{{"used_percent":42.0,"window_minutes":300,"resets_at":{resets_at}}},"secondary":{{"used_percent":7.5,"window_minutes":10080,"resets_at":null}}}}}}}}"#
        ),
    ]
    .join("\n");
    fs::write(codex.join("rollout-x.jsonl"), body).unwrap();
}

fn t(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Value {
    json!({"input": input, "output": output, "cache_read": cache_read,
           "cache_write": cache_write, "total": input + output + cache_read + cache_write})
}

#[test]
fn usage_json_has_the_contract_shape() {
    let env = Env::isolated();
    write_fixtures(&env);
    let (code, v) = env.json(&["usage", "--json"], None, &[("TZ", "UTC")]);
    let d = ok_data(code, &v);
    assert_eq!(d["days"], 7);
    assert!(d["generated_at"].as_u64().unwrap() > 1_700_000_000);
    let agents = d["agents"].as_array().unwrap();
    let names: Vec<_> = agents
        .iter()
        .map(|a| a["agent"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["claude", "codex", "gemini", "cursor", "copilot", "kiro", "opencode", "aider"]
    );

    let claude_total = t(11, 21, 41, 31);
    assert_eq!(
        agents[0],
        json!({
            "agent": "claude", "available": true,
            "note": "plan limits are not recorded locally",
            "today": claude_total, "period": claude_total,
            "projects": [{"project": "OpenMosh", "tokens": t(10, 20, 40, 30)},
                         {"project": "Other", "tokens": t(1, 1, 1, 1)}],
            "models": [{"model": "claude-opus", "tokens": claude_total}],
            "limits": []
        })
    );
    let codex_total = t(200, 50, 800, 0);
    let resets_at = &agents[1]["limits"][0]["resets_at"];
    assert!(resets_at.is_u64());
    assert_eq!(
        agents[1],
        json!({
            "agent": "codex", "available": true, "note": null,
            "today": codex_total, "period": codex_total,
            "projects": [{"project": "App", "tokens": codex_total}],
            "models": [{"model": "gpt-5", "tokens": codex_total}],
            "limits": [
                {"name": "5h", "used_percent": 42.0, "window_minutes": 300, "resets_at": resets_at},
                {"name": "weekly", "used_percent": 7.5, "window_minutes": 10080, "resets_at": null}
            ]
        })
    );
    assert_eq!(
        agents[2],
        json!({
            "agent": "gemini", "available": false, "note": "usage not recorded locally",
            "today": t(0, 0, 0, 0), "period": t(0, 0, 0, 0),
            "projects": [], "models": [], "limits": []
        })
    );
    assert!(!v.to_string().contains("do not print me"));
}

#[test]
fn usage_without_logs_reports_every_agent_unavailable() {
    let env = Env::isolated();
    let (code, v) = env.json(&["usage", "--days", "1", "--json"], None, &[]);
    let d = ok_data(code, &v);
    assert_eq!(d["days"], 1);
    assert_eq!(d["agents"][0]["available"], false);
    assert_eq!(d["agents"][0]["note"], "no local logs found");
}

#[test]
fn usage_days_outside_1_to_31_is_refused() {
    let env = Env::isolated();
    for bad in ["0", "32", "-1"] {
        let (code, v) = env.json(&["usage", "--days", bad, "--json"], None, &[]);
        assert_eq!(err_code(code, &v), "invalid_input");
        assert_eq!(v["error"]["message"], "--days must be between 1 and 31");
    }
    let out = env.run(&["usage", "--days", "40"], None, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--days must be between 1 and 31"));
}

#[test]
fn usage_human_output_lists_each_agent() {
    let env = Env::isolated();
    write_fixtures(&env);
    let out = env.run(&["usage"], None, &[("TZ", "UTC")]);
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("claude\n  today     104 (in 11"), "{s}");
    assert!(s.contains("  projects  OpenMosh 100, Other 4"), "{s}");
    assert!(
        s.contains("  limit     5h 42% used, resets in 0h59m") || s.contains("resets in 1h00m"),
        "{s}"
    );
    assert!(s.contains("gemini    usage not recorded locally"), "{s}");
    assert!(!s.contains("do not print me"));
}
