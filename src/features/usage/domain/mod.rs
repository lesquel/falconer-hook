//! Token usage per agent, project and model, read from the agents' own
//! local logs. Pure: parsing, the time window and aggregation.

mod parse;
mod time;

pub use parse::{CodexParser, CodexSession};
#[cfg(test)]
use time::parse_rfc3339;
pub use time::parse_utc_offset;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::HashMap;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

const DAY: i64 = 86_400;
const TOP_PROJECTS: usize = 10;
const TOP_MODELS: usize = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UsageError {
    #[error("--days must be between 1 and 31")]
    InvalidDays,
}

impl ErrorCode for UsageError {
    fn code(&self) -> &'static str {
        "invalid_input"
    }
}

/// Length of the period in calendar days, today included (1..=31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days(u32);

impl Days {
    pub fn new(n: i64) -> Result<Self, UsageError> {
        match u32::try_from(n) {
            Ok(n @ 1..=31) => Ok(Self(n)),
            _ => Err(UsageError::InvalidDays),
        }
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Bounds in unix seconds: "today" since local midnight, the period since
/// local midnight `days - 1` days ago.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub now: u64,
    pub today_start: u64,
    pub period_start: u64,
    pub days: Days,
}

impl Window {
    /// `utc_offset` is the local offset from UTC in seconds (east positive).
    pub fn new(now: u64, utc_offset: i64, days: Days) -> Self {
        let local = now as i64 + utc_offset;
        let today = local.div_euclid(DAY) * DAY - utc_offset;
        let period = today - (i64::from(days.get()) - 1) * DAY;
        Self {
            now,
            today_start: today.max(0) as u64,
            period_start: period.max(0) as u64,
            days,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
    fn add(&mut self, o: &Tokens) {
        self.input = self.input.saturating_add(o.input);
        self.output = self.output.saturating_add(o.output);
        self.cache_read = self.cache_read.saturating_add(o.cache_read);
        self.cache_write = self.cache_write.saturating_add(o.cache_write);
    }
}

impl Serialize for Tokens {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(5))?;
        m.serialize_entry("input", &self.input)?;
        m.serialize_entry("output", &self.output)?;
        m.serialize_entry("cache_read", &self.cache_read)?;
        m.serialize_entry("cache_write", &self.cache_write)?;
        m.serialize_entry("total", &self.total())?;
        m.end()
    }
}

/// Tokens attributed to one moment (for Codex, one session at its last event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub at: u64,
    pub project: Option<String>,
    pub model: Option<String>,
    pub tokens: Tokens,
}

/// Plan usage as the agent itself last recorded it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limit {
    pub name: String,
    pub used_percent: f64,
    pub window_minutes: u64,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectTokens {
    pub project: String,
    pub tokens: Tokens,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelTokens {
    pub model: String,
    pub tokens: Tokens,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentUsage {
    pub agent: String,
    pub available: bool,
    pub note: Option<String>,
    pub today: Tokens,
    pub period: Tokens,
    pub projects: Vec<ProjectTokens>,
    pub models: Vec<ModelTokens>,
    pub limits: Vec<Limit>,
}

impl AgentUsage {
    /// An agent whose usage cannot be read on this machine.
    pub fn unavailable(agent: &str, note: &str) -> Self {
        Self {
            agent: agent.into(),
            available: false,
            note: Some(note.into()),
            today: Tokens::default(),
            period: Tokens::default(),
            projects: vec![],
            models: vec![],
            limits: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageReport {
    pub days: u32,
    pub generated_at: u64,
    pub agents: Vec<AgentUsage>,
}

/// Running totals for one agent over a window.
#[derive(Debug)]
pub struct Tally {
    window: Window,
    today: Tokens,
    period: Tokens,
    projects: HashMap<String, Tokens>,
    models: HashMap<String, Tokens>,
}

impl Tally {
    pub fn new(window: Window) -> Self {
        Self {
            window,
            today: Tokens::default(),
            period: Tokens::default(),
            projects: HashMap::new(),
            models: HashMap::new(),
        }
    }

    /// Counts `r` when it falls inside the period.
    pub fn add(&mut self, r: Record) {
        if r.at < self.window.period_start {
            return;
        }
        self.period.add(&r.tokens);
        if r.at >= self.window.today_start {
            self.today.add(&r.tokens);
        }
        if let Some(p) = r.project {
            self.projects.entry(p).or_default().add(&r.tokens);
        }
        if let Some(m) = r.model {
            self.models.entry(m).or_default().add(&r.tokens);
        }
    }

    pub fn finish(self, agent: &str, note: Option<String>, limits: Vec<Limit>) -> AgentUsage {
        AgentUsage {
            agent: agent.into(),
            available: true,
            note,
            today: self.today,
            period: self.period,
            projects: top(self.projects, TOP_PROJECTS)
                .map(|(project, tokens)| ProjectTokens { project, tokens })
                .collect(),
            models: top(self.models, TOP_MODELS)
                .map(|(model, tokens)| ModelTokens { model, tokens })
                .collect(),
            limits,
        }
    }
}

/// The `n` largest entries by total, ties broken by name.
fn top(map: HashMap<String, Tokens>, n: usize) -> impl Iterator<Item = (String, Tokens)> {
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(&b.0)));
    v.into_iter().take(n)
}

#[cfg(test)]
mod tests;
