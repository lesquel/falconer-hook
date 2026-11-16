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

const CLAUDE_NOTE: &str =
    "not read: Claude Code keeps no usage summary, and its transcripts are too large to scan";

fn count(ts: &str, total: [u64; 3], limits: &str) -> String {
    format!(
        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"total_tokens":{}}},"last_token_usage":{{"input_tokens":1,"output_tokens":1}}}},"rate_limits":{limits}}}}}"#,
        total[0],
        total[1],
        total[2],
        total[0] + total[2]
    )
}

fn turn(ts: &str, cwd: &str, model: &str) -> String {
    format!(
        r#"{{"timestamp":"{ts}","type":"turn_context","payload":{{"cwd":"{cwd}","model":"{model}"}}}}"#
    )
}

/// Claude transcripts that must not be opened (unreadable), and two Codex
/// rollouts: a 1 MiB one whose middle would change the model and the totals
/// if it were read, and a small one read whole.
fn write_fixtures(env: &Env) {
    use std::os::unix::fs::PermissionsExt;
    let ts = now_rfc3339();
    let home = env.home();
    let claude = home.join(".claude/projects/-w-Falconer");
    fs::create_dir_all(&claude).unwrap();
    let transcript = claude.join("s.jsonl");
    fs::write(
        &transcript,
        r#"{"type":"assistant","timestamp":"2026-11-08T12:00:00Z","message":{"usage":{"input_tokens":9}}}"#,
    )
    .unwrap();
    fs::set_permissions(&transcript, fs::Permissions::from_mode(0o000)).unwrap();

    let codex = home.join(".codex/sessions/2026/11/08");
    fs::create_dir_all(&codex).unwrap();
    let resets_at = utc(&["+%s"]).parse::<u64>().unwrap() + 3600;
    let limits = format!(
        r#"{{"primary":{{"used_percent":42.0,"window_minutes":300,"resets_at":{resets_at}}},"secondary":{{"used_percent":7.5,"window_minutes":10080,"resets_at":null}}}}"#
    );
    let filler = format!(
        r#"{{"timestamp":"{ts}","type":"response_item","payload":{{"text":"do not print me {}"}}}}"#,
        "x".repeat(1000)
    );
    let mut big = vec![
        format!(
            r#"{{"timestamp":"{ts}","type":"session_meta","payload":{{"id":"s","cwd":"/w/App"}}}}"#
        ),
        turn(&ts, "/w/App", "gpt-5"),
    ];
    big.extend(std::iter::repeat_n(filler.clone(), 400));
    big.push(turn(&ts, "/w/Middle", "gpt-middle"));
    big.push(count(
        &ts,
        [9_000_000, 0, 0],
        &limits.replace("42.0", "99.0"),
    ));
    big.extend(std::iter::repeat_n(filler.clone(), 600));
    big.push(count(&ts, [500, 0, 10], "null"));
    big.push(count(&ts, [1000, 800, 50], &limits));
    let big = big.join("\n");
    assert!(big.len() > 1024 * 1024);
    fs::write(codex.join("rollout-big.jsonl"), big).unwrap();

    let small = [
        turn(&ts, "/w/Small", "gpt-5-mini"),
        filler,
        count(
            "2020-01-01T00:00:00Z",
            [1, 0, 1],
            &limits.replace("42.0", "1.0"),
        ),
        count(&ts, [30, 10, 4], "null"),
    ]
    .join("\n");
    fs::write(codex.join("rollout-small.jsonl"), small).unwrap();
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

    assert_eq!(
        agents[0],
        json!({
            "agent": "claude", "available": false, "note": CLAUDE_NOTE,
            "today": t(0, 0, 0, 0), "period": t(0, 0, 0, 0),
            "projects": [], "models": [], "limits": []
        })
    );
    let app = t(200, 50, 800, 0);
    let small = t(20, 4, 10, 0);
    let codex_total = t(220, 54, 810, 0);
    let resets_at = &agents[1]["limits"][0]["resets_at"];
    assert!(resets_at.is_u64());
    assert_eq!(
        agents[1],
        json!({
            "agent": "codex", "available": true, "note": null,
            "today": codex_total, "period": codex_total,
            "projects": [{"project": "App", "tokens": app},
                         {"project": "Small", "tokens": small}],
            "models": [{"model": "gpt-5", "tokens": app},
                       {"model": "gpt-5-mini", "tokens": small}],
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
    assert_eq!(d["agents"][0]["note"], CLAUDE_NOTE);
    assert_eq!(d["agents"][1]["available"], false);
    assert_eq!(d["agents"][1]["note"], "no local logs found");
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
    assert!(s.contains(&format!("claude    {CLAUDE_NOTE}")), "{s}");
    assert!(s.contains("codex\n  today     1.1k (in 220"), "{s}");
    assert!(s.contains("  projects  App 1.1k, Small 34"), "{s}");
    assert!(
        s.contains("  limit     5h 42% used, resets in 0h59m") || s.contains("resets in 1h00m"),
        "{s}"
    );
    assert!(s.contains("gemini    usage not recorded locally"), "{s}");
    assert!(!s.contains("do not print me"));
}
