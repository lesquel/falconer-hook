//! Line parser for Codex's JSONL rollouts. Lines that cannot carry usage
//! are dropped by a substring check before any JSON parsing; malformed lines
//! are ignored. Message text is never kept.

use serde::Deserialize;
use serde_json::Value;

use super::time::parse_rfc3339;
use super::{Limit, Record, Tokens};
use crate::shared::domain::text::{clean_label, project_of};

fn model_label(m: &str) -> Option<String> {
    Some(clean_label(m, 64)).filter(|m| !m.is_empty())
}

/// What one Codex rollout file holds: the session's cumulative usage at its
/// last `token_count` event, and the newest plan limits seen in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexSession {
    pub record: Option<Record>,
    pub limits: Option<(u64, Vec<Limit>)>,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Value>,
}

/// Codex rollouts (`~/.codex/sessions/**/rollout-*.jsonl`), fed only the head
/// and the tail of each file. `session_meta` and `turn_context` set the
/// working directory and model; every `token_count` carries the session's
/// running total, so the last one is the whole session. `take` ends a file.
#[derive(Debug, Default)]
pub struct CodexParser {
    cwd: Option<String>,
    model: Option<String>,
    last: Option<(u64, Tokens)>,
    limits: Option<(u64, Vec<Limit>)>,
}

impl CodexParser {
    pub fn feed(&mut self, line: &str) {
        if !["\"token_count\"", "\"turn_context\"", "\"session_meta\""]
            .iter()
            .any(|k| line.contains(k))
        {
            return;
        }
        let Ok(l) = serde_json::from_str::<CodexLine>(line) else {
            return;
        };
        let (Some(p), Some(kind)) = (l.payload, l.kind) else {
            return;
        };
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match kind.as_str() {
            "session_meta" => self.cwd = text(&p, "cwd").or(self.cwd.take()),
            "turn_context" => {
                self.cwd = text(&p, "cwd").or(self.cwd.take());
                self.model = text(&p, "model").or(self.model.take());
            }
            "event_msg" if p.get("type").and_then(Value::as_str) == Some("token_count") => {
                let Some(at) = l.timestamp.as_deref().and_then(parse_rfc3339) else {
                    return;
                };
                if let Some(t) = total_usage(&p) {
                    self.last = Some((at, t));
                }
                if let Some(v) = p.get("rate_limits").and_then(|r| limits(r, at)) {
                    if self.limits.as_ref().is_none_or(|(t, _)| at >= *t) {
                        self.limits = Some((at, v));
                    }
                }
            }
            _ => {}
        }
    }

    /// The session read so far; the parser starts over for the next file.
    pub fn take(&mut self) -> CodexSession {
        let s = std::mem::take(self);
        CodexSession {
            record: s
                .last
                .filter(|(_, t)| t.total() > 0)
                .map(|(at, tokens)| Record {
                    at,
                    project: s.cwd.as_deref().and_then(project_of),
                    model: s.model.as_deref().and_then(model_label),
                    tokens,
                }),
            limits: s.limits,
        }
    }
}

/// `info.total_token_usage` of a `token_count` payload.
fn total_usage(p: &Value) -> Option<Tokens> {
    let total = p.get("info")?.get("total_token_usage")?;
    if !total.is_object() {
        return None;
    }
    let n = |k: &str| total.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cached_input_tokens");
    Some(Tokens {
        input: n("input_tokens").saturating_sub(cached),
        output: n("output_tokens"),
        cache_read: cached,
        cache_write: 0,
    })
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
