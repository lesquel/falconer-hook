//! Line parsers for the agents' JSONL logs. Lines that cannot carry usage
//! are dropped by a substring check before any JSON parsing; malformed lines
//! are ignored. Message text is never kept.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

use super::time::parse_rfc3339;
use super::{Limit, Record, Tokens};
use crate::shared::domain::text::{clean_label, project_of};

fn model_label(m: &str) -> Option<String> {
    Some(clean_label(m, 64)).filter(|m| !m.is_empty())
}

#[derive(Deserialize)]
struct ClaudeLine {
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

/// Claude Code transcripts (`~/.claude/projects/**/*.jsonl`). A streamed
/// response is written once per content block with the same `message.id` and
/// `requestId`; it counts once across every file fed to the same parser.
#[derive(Debug, Default)]
pub struct ClaudeParser {
    seen: HashSet<String>,
}

impl ClaudeParser {
    pub fn feed(&mut self, line: &str) -> Option<Record> {
        if !line.contains("\"usage\"") {
            return None;
        }
        let l: ClaudeLine = serde_json::from_str(line).ok()?;
        let msg = l.message?;
        let u = msg.usage?;
        let at = parse_rfc3339(l.timestamp.as_deref()?)?;
        let tokens = Tokens {
            input: u.input_tokens.unwrap_or(0),
            output: u.output_tokens.unwrap_or(0),
            cache_read: u.cache_read_input_tokens.unwrap_or(0),
            cache_write: u.cache_creation_input_tokens.unwrap_or(0),
        };
        if tokens.total() == 0 {
            return None;
        }
        if msg.id.is_some() || l.request_id.is_some() {
            let key = format!(
                "{}\u{1f}{}",
                msg.id.as_deref().unwrap_or(""),
                l.request_id.as_deref().unwrap_or("")
            );
            if !self.seen.insert(key) {
                return None;
            }
        }
        Some(Record {
            at,
            project: l.cwd.as_deref().and_then(project_of),
            model: msg.model.as_deref().and_then(model_label),
            tokens,
        })
    }
}

/// One Codex `token_count` event: the response's usage and/or the plan
/// limits Codex received with it.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexEvent {
    pub at: u64,
    pub record: Option<Record>,
    pub limits: Option<Vec<Limit>>,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Value>,
}

/// Codex rollouts (`~/.codex/sessions/**/rollout-*.jsonl`). `session_meta`
/// and `turn_context` set the working directory and model for the
/// `token_count` events that follow; call `reset` before each file.
#[derive(Debug, Default)]
pub struct CodexParser {
    cwd: Option<String>,
    model: Option<String>,
    last_total: Option<u64>,
}

impl CodexParser {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn feed(&mut self, line: &str) -> Option<CodexEvent> {
        if !["\"token_count\"", "\"turn_context\"", "\"session_meta\""]
            .iter()
            .any(|k| line.contains(k))
        {
            return None;
        }
        let l: CodexLine = serde_json::from_str(line).ok()?;
        let p = l.payload?;
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match l.kind.as_deref()? {
            "session_meta" => {
                self.cwd = text(&p, "cwd").or(self.cwd.take());
                None
            }
            "turn_context" => {
                self.cwd = text(&p, "cwd").or(self.cwd.take());
                self.model = text(&p, "model").or(self.model.take());
                None
            }
            "event_msg" if p.get("type").and_then(Value::as_str) == Some("token_count") => {
                let at = parse_rfc3339(l.timestamp.as_deref()?)?;
                let record = self.record(&p, at);
                let limits = p.get("rate_limits").and_then(|r| limits(r, at));
                (record.is_some() || limits.is_some()).then_some(CodexEvent { at, record, limits })
            }
            _ => None,
        }
    }

    fn record(&mut self, p: &Value, at: u64) -> Option<Record> {
        let info = p.get("info").filter(|i| i.is_object())?;
        let total = info
            .get("total_token_usage")
            .and_then(|t| t.get("total_tokens"))
            .and_then(Value::as_u64);
        // Codex repeats the last count (e.g. for a rate-limit update).
        if total.is_some() && total == self.last_total {
            return None;
        }
        self.last_total = total;
        let last = info.get("last_token_usage")?;
        let n = |k: &str| last.get(k).and_then(Value::as_u64).unwrap_or(0);
        let cached = n("cached_input_tokens");
        let tokens = Tokens {
            input: n("input_tokens").saturating_sub(cached),
            output: n("output_tokens"),
            cache_read: cached,
            cache_write: 0,
        };
        (tokens.total() > 0).then(|| Record {
            at,
            project: self.cwd.as_deref().and_then(project_of),
            model: self.model.as_deref().and_then(model_label),
            tokens,
        })
    }
}

fn limits(r: &Value, at: u64) -> Option<Vec<Limit>> {
    let v: Vec<Limit> = [
        ("primary", r.get("primary")),
        ("secondary", r.get("secondary")),
    ]
    .into_iter()
    .filter_map(|(slot, l)| {
        let l = l?;
        let used_percent = l.get("used_percent")?.as_f64().filter(|p| p.is_finite())?;
        let window_minutes = l.get("window_minutes").and_then(Value::as_u64).unwrap_or(0);
        let name = match window_minutes {
            300 => "5h",
            10_080 => "weekly",
            _ => slot,
        };
        let resets_at = l.get("resets_at").and_then(Value::as_u64).or_else(|| {
            l.get("resets_in_seconds")
                .and_then(Value::as_u64)
                .map(|s| at.saturating_add(s))
        });
        Some(Limit {
            name: name.into(),
            used_percent,
            window_minutes,
            resets_at,
        })
    })
    .collect();
    (!v.is_empty()).then_some(v)
}
