use super::*;

const DAY: u64 = 86_400;
// 2026-11-08T12:00:00Z
const NOW: u64 = 1_794_139_200;

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
    let u = t.finish("codex", None, vec![]);
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
    let u = t.finish("codex", None, vec![]);
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

/// A `token_count` event; `total` is the session's running usage
/// (input, cached input, output).
fn codex_count(ts: &str, total: [u64; 3], limits: &str) -> String {
    format!(
        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"reasoning_output_tokens":5,"total_tokens":{}}},"last_token_usage":{{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1,"total_tokens":2}}}},"rate_limits":{limits}}}}}"#,
        total[0],
        total[1],
        total[2],
        total[0] + total[2]
    )
}

const LIMITS: &str = r#"{"primary":{"used_percent":42.5,"window_minutes":300,"resets_at":1794150000},"secondary":{"used_percent":7.0,"window_minutes":10080,"resets_in_seconds":3600}}"#;

#[test]
fn codex_session_total_is_its_last_token_count_with_cached_input_as_cache_read() {
    let mut p = CodexParser::default();
    p.feed(&codex_meta("/w/Old"));
    p.feed(&codex_turn("/w/App", "gpt-5"));
    p.feed(&codex_count("2026-11-08T11:30:00Z", [100, 50, 5], "null"));
    p.feed(&codex_count(
        "2026-11-08T12:00:00Z",
        [1000, 800, 50],
        "null",
    ));
    // A rate-limit-only update after the last count keeps the total.
    p.feed(r#"{"timestamp":"2026-11-08T12:05:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}"#);
    let s = p.take();
    let r = s.record.unwrap();
    assert_eq!(r.at, NOW);
    assert_eq!(r.project.as_deref(), Some("App"));
    assert_eq!(r.model.as_deref(), Some("gpt-5"));
    assert_eq!(r.tokens, tokens(200, 50, 800, 0));
    assert!(s.limits.is_none());
}

#[test]
fn codex_take_starts_the_next_file_over() {
    let mut p = CodexParser::default();
    p.feed(&codex_turn("/w/App", "gpt-5"));
    p.feed(&codex_count("2026-11-08T12:00:00Z", [10, 0, 1], LIMITS));
    assert!(p.take().record.is_some());
    assert_eq!(p.take(), CodexSession::default());
    p.feed(&codex_count("2026-11-08T12:00:00Z", [10, 0, 1], "null"));
    let r = p.take().record.unwrap();
    assert_eq!((r.project, r.model), (None, None));
}

#[test]
fn codex_rate_limits_become_named_limits_and_the_newest_in_a_file_wins() {
    let mut p = CodexParser::default();
    p.feed(&codex_count("2026-11-08T12:00:00Z", [10, 0, 1], LIMITS));
    let odd = r#"{"primary":{"used_percent":1,"window_minutes":60},"secondary":null}"#;
    p.feed(&codex_count("2026-11-08T11:00:00Z", [10, 0, 1], odd));
    let (at, limits) = p.take().limits.unwrap();
    assert_eq!(at, NOW);
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
    p.feed(&codex_count("2026-11-08T12:00:00Z", [1, 0, 1], odd));
    let (_, limits) = p.take().limits.unwrap();
    assert_eq!(
        (limits[0].name.as_str(), limits[0].resets_at),
        ("primary", None)
    );
}

#[test]
fn codex_ignores_malformed_lines_counts_without_info_and_empty_sessions() {
    let mut p = CodexParser::default();
    p.feed("{\"token_count\" broken");
    p.feed(r#"{"timestamp":"2026-11-08T12:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}"#);
    p.feed(r#"{"type":"response_item","payload":{"text":"hi"}}"#);
    // No timestamp: dropped.
    p.feed(r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5}}}}"#);
    assert_eq!(p.take(), CodexSession::default());
    p.feed(&codex_count("2026-11-08T12:00:00Z", [0, 0, 0], "null"));
    assert_eq!(p.take().record, None, "a session with no tokens");
}
