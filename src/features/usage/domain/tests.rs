use super::*;

const DAY: u64 = 86_400;
// 2026-11-08T12:00:00Z
const NOW: u64 = 1_794_139_200;

fn claude_line(id: &str, req: &str, ts: &str, cwd: &str, model: &str, u: [u64; 4]) -> String {
    format!(
        r#"{{"type":"assistant","cwd":"{cwd}","requestId":"{req}","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","content":[{{"type":"text","text":"secret"}}],"usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":{},"cache_read_input_tokens":{}}}}}}}"#,
        u[0], u[1], u[2], u[3]
    )
}

fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Tokens {
    Tokens {
        input,
        output,
        cache_read,
        cache_write,
    }
}

fn rec(at: u64, project: &str, model: &str, t: Tokens) -> Record {
    Record {
        at,
        project: Some(project.into()),
        model: Some(model.into()),
        tokens: t,
    }
}

#[test]
fn rfc3339_timestamps_parse_to_unix_seconds() {
    assert_eq!(parse_rfc3339("2026-11-08T12:00:00Z"), Some(NOW));
    assert_eq!(parse_rfc3339("2026-11-08T12:00:00.987Z"), Some(NOW));
    assert_eq!(parse_rfc3339("2026-11-08T14:30:00+02:30"), Some(NOW));
    assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_rfc3339("2026-13-08T12:00:00Z"), None);
    assert_eq!(parse_rfc3339("yesterday"), None);
}

#[test]
fn utc_offsets_parse_from_date_output() {
    assert_eq!(parse_utc_offset("+0530\n"), Some(19_800));
    assert_eq!(parse_utc_offset("-0500"), Some(-18_000));
    assert_eq!(parse_utc_offset("+0000"), Some(0));
    assert_eq!(parse_utc_offset("EST"), None);
}

#[test]
fn days_accepts_1_to_31_only() {
    assert_eq!(Days::new(1).unwrap().get(), 1);
    assert_eq!(Days::new(31).unwrap().get(), 31);
    for bad in [0, 32, -3] {
        let e = Days::new(bad).unwrap_err();
        assert_eq!(e.code(), "invalid_input");
        assert_eq!(e.to_string(), "--days must be between 1 and 31");
    }
}

#[test]
fn today_starts_at_local_midnight_and_the_period_covers_n_calendar_days() {
    // UTC: midnight of 2026-11-08.
    let w = Window::new(NOW, 0, Days::new(7).unwrap());
    assert_eq!(w.today_start, NOW - 12 * 3600);
    assert_eq!(w.period_start, w.today_start - 6 * DAY);
    // UTC-5: local time is 07:00, local midnight is 05:00Z.
    let w = Window::new(NOW, -18_000, Days::new(1).unwrap());
    assert_eq!(w.today_start, NOW - 7 * 3600);
    assert_eq!(w.period_start, w.today_start);
    // UTC+14: local time is 02:00 on the 9th, local midnight is 10:00Z.
    let w = Window::new(NOW, 14 * 3600, Days::new(2).unwrap());
    assert_eq!(w.today_start, NOW - 2 * 3600);
}

#[test]
fn tally_splits_today_from_the_period_around_local_midnight() {
    let w = Window::new(NOW, -18_000, Days::new(7).unwrap());
    let mut t = Tally::new(w);
    let before_midnight = w.today_start - 1;
    t.add(rec(before_midnight, "a", "m", tokens(1, 0, 0, 0)));
    t.add(rec(w.today_start, "a", "m", tokens(10, 0, 0, 0)));
    t.add(rec(w.period_start - 1, "a", "m", tokens(100, 0, 0, 0)));
    let u = t.finish("claude", None, vec![]);
    assert_eq!(u.today, tokens(10, 0, 0, 0));
    assert_eq!(u.period, tokens(11, 0, 0, 0));
}

#[test]
fn tally_keeps_top_10_projects_and_top_5_models_by_period_total() {
    let w = Window::new(NOW, 0, Days::new(7).unwrap());
    let mut t = Tally::new(w);
    for i in 1..=12u64 {
        let name = format!("p{i:02}");
        t.add(rec(NOW, &name, &format!("m{i:02}"), tokens(i, 0, 0, 0)));
    }
    // A tie is broken by name.
    t.add(rec(NOW, "p00", "m00", tokens(12, 0, 0, 0)));
    let u = t.finish("claude", None, vec![]);
    let projects: Vec<_> = u.projects.iter().map(|p| p.project.as_str()).collect();
    assert_eq!(
        projects,
        ["p00", "p12", "p11", "p10", "p09", "p08", "p07", "p06", "p05", "p04"]
    );
    assert_eq!(u.projects[0].tokens.total(), 12);
    let models: Vec<_> = u.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(models, ["m00", "m12", "m11", "m10", "m09"]);
}

#[test]
fn tokens_serialize_with_their_total() {
    let v = serde_json::to_value(tokens(1, 2, 3, 4)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"input":1,"output":2,"cache_read":3,"cache_write":4,"total":10})
    );
}

#[test]
fn claude_lines_map_tokens_and_repeated_stream_messages_count_once() {
    let mut p = ClaudeParser::default();
    let l = claude_line(
        "msg_1",
        "req_1",
        "2026-11-08T12:00:00Z",
        "/w/OpenMosh",
        "claude-opus",
        [2, 493, 60_636, 28_196],
    );
    let r = p.feed(&l).unwrap();
    assert_eq!(r.at, NOW);
    assert_eq!(r.project.as_deref(), Some("OpenMosh"));
    assert_eq!(r.model.as_deref(), Some("claude-opus"));
    assert_eq!(r.tokens, tokens(2, 493, 28_196, 60_636));
    assert!(p.feed(&l).is_none(), "streaming repeat counted twice");
    let other_request = claude_line(
        "msg_1",
        "req_2",
        "2026-11-08T12:00:00Z",
        "/w",
        "m",
        [1, 1, 0, 0],
    );
    assert!(p.feed(&other_request).is_some());
}

#[test]
fn claude_ignores_malformed_unrelated_and_empty_lines() {
    let mut p = ClaudeParser::default();
    assert!(p.feed("{not json \"usage\"").is_none());
    assert!(p
        .feed(r#"{"type":"user","message":{"content":"hi"}}"#)
        .is_none());
    assert!(
        p.feed(r#"{"message":{"id":"x","usage":{"input_tokens":1}}}"#)
            .is_none(),
        "no timestamp"
    );
    let zero = claude_line(
        "m",
        "r",
        "2026-11-08T12:00:00Z",
        "/w",
        "<synthetic>",
        [0, 0, 0, 0],
    );
    assert!(p.feed(&zero).is_none());
}

fn codex_meta(cwd: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-11-08T11:00:00Z","type":"session_meta","payload":{{"id":"s","cwd":"{cwd}","base_instructions":{{"text":"secret"}}}}}}"#
    )
}

fn codex_turn(cwd: &str, model: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-11-08T11:00:01Z","type":"turn_context","payload":{{"cwd":"{cwd}","model":"{model}"}}}}"#
    )
}

fn codex_count(ts: &str, last: [u64; 3], total: u64, limits: &str) -> String {
    format!(
        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"reasoning_output_tokens":5,"total_tokens":{}}}}},"rate_limits":{limits}}}}}"#,
        last[0],
        last[1],
        last[2],
        last[0] + last[2]
    )
}

const LIMITS: &str = r#"{"primary":{"used_percent":42.5,"window_minutes":300,"resets_at":1794150000},"secondary":{"used_percent":7.0,"window_minutes":10080,"resets_in_seconds":3600}}"#;

#[test]
fn codex_token_counts_map_cached_input_to_cache_read() {
    let mut p = CodexParser::default();
    assert!(p.feed(&codex_meta("/w/Old")).is_none());
    assert!(p.feed(&codex_turn("/w/App", "gpt-5")).is_none());
    let ev = p
        .feed(&codex_count(
            "2026-11-08T12:00:00Z",
            [1000, 800, 50],
            1050,
            "null",
        ))
        .unwrap();
    let r = ev.record.unwrap();
    assert_eq!(r.at, NOW);
    assert_eq!(r.project.as_deref(), Some("App"));
    assert_eq!(r.model.as_deref(), Some("gpt-5"));
    assert_eq!(r.tokens, tokens(200, 50, 800, 0));
    assert!(ev.limits.is_none());
}

#[test]
fn codex_repeated_token_count_with_the_same_total_counts_once() {
    let mut p = CodexParser::default();
    let l = codex_count("2026-11-08T12:00:00Z", [10, 0, 1], 11, "null");
    assert!(p.feed(&l).unwrap().record.is_some());
    assert!(p.feed(&l).is_none_or(|e| e.record.is_none()));
    p.reset();
    assert!(
        p.feed(&l).unwrap().record.is_some(),
        "a new file starts over"
    );
}

#[test]
fn codex_rate_limits_become_named_limits() {
    let mut p = CodexParser::default();
    let ev = p
        .feed(&codex_count("2026-11-08T12:00:00Z", [10, 0, 1], 11, LIMITS))
        .unwrap();
    let limits = ev.limits.unwrap();
    assert_eq!(ev.at, NOW);
    assert_eq!(
        limits,
        vec![
            Limit {
                name: "5h".into(),
                used_percent: 42.5,
                window_minutes: 300,
                resets_at: Some(1_794_150_000),
            },
            Limit {
                name: "weekly".into(),
                used_percent: 7.0,
                window_minutes: 10_080,
                resets_at: Some(NOW + 3600),
            },
        ]
    );
    let odd = r#"{"primary":{"used_percent":1,"window_minutes":60},"secondary":null}"#;
    let ev = p
        .feed(&codex_count("2026-11-08T12:00:00Z", [1, 0, 1], 99, odd))
        .unwrap();
    let l = &ev.limits.unwrap()[0];
    assert_eq!((l.name.as_str(), l.resets_at), ("primary", None));
}

#[test]
fn codex_ignores_malformed_lines_and_counts_without_info() {
    let mut p = CodexParser::default();
    assert!(p.feed("{\"token_count\" broken").is_none());
    let no_info = r#"{"timestamp":"2026-11-08T12:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}"#;
    assert!(p
        .feed(no_info)
        .is_none_or(|e| e.record.is_none() && e.limits.is_none()));
    assert!(p
        .feed(r#"{"type":"response_item","payload":{"text":"hi"}}"#)
        .is_none());
}
