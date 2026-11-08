//! Use case: the usage report over the agents' local logs.

use crate::features::usage::domain::{
    AgentUsage, ClaudeParser, CodexParser, Days, Limit, Tally, UsageReport, Window,
};
use crate::shared::domain::clock::Clock;

/// Agents that keep usage logs this hook can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogAgent {
    Claude,
    Codex,
}

/// What a scan hands to its visitor, file by file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLine<'a> {
    /// A new file starts; the lines that follow belong to it.
    File,
    Line(&'a str),
}

/// Port over the agents' log directories (read-only).
pub trait UsageLogs {
    /// Visits every line of the agent's log files modified at or after
    /// `modified_since`. Returns `false` when the agent has no log directory.
    fn scan(
        &self,
        agent: LogAgent,
        modified_since: u64,
        visit: &mut dyn FnMut(LogLine<'_>),
    ) -> bool;
}

/// Supported agents that record no token usage on disk.
pub const NOT_RECORDED: [&str; 6] = ["gemini", "cursor", "copilot", "kiro", "opencode", "aider"];

pub struct ReportUsage<'a> {
    pub logs: &'a dyn UsageLogs,
    pub clock: &'a dyn Clock,
    /// Local offset from UTC in seconds, for "today".
    pub utc_offset: i64,
}

impl ReportUsage<'_> {
    pub fn run(&self, days: Days) -> UsageReport {
        let now = self.clock.now();
        let window = Window::new(now, self.utc_offset, days);
        let mut agents = vec![self.claude(window), self.codex(window)];
        agents.extend(
            NOT_RECORDED
                .iter()
                .map(|a| AgentUsage::unavailable(a, "usage not recorded locally")),
        );
        UsageReport {
            days: days.get(),
            generated_at: now,
            agents,
        }
    }

    fn claude(&self, window: Window) -> AgentUsage {
        let (mut parser, mut tally) = (ClaudeParser::default(), Tally::new(window));
        let found = self
            .logs
            .scan(LogAgent::Claude, window.period_start, &mut |l| {
                if let LogLine::Line(line) = l {
                    if let Some(r) = parser.feed(line) {
                        tally.add(r);
                    }
                }
            });
        if !found {
            return AgentUsage::unavailable("claude", "no local logs found");
        }
        // Claude Code keeps no plan usage on disk; it is never fetched.
        tally.finish(
            "claude",
            Some("plan limits are not recorded locally".into()),
            vec![],
        )
    }

    fn codex(&self, window: Window) -> AgentUsage {
        let (mut parser, mut tally) = (CodexParser::default(), Tally::new(window));
        let mut newest: Option<(u64, Vec<Limit>)> = None;
        let found = self
            .logs
            .scan(LogAgent::Codex, window.period_start, &mut |l| match l {
                LogLine::File => parser.reset(),
                LogLine::Line(line) => {
                    let Some(ev) = parser.feed(line) else { return };
                    if let Some(r) = ev.record {
                        tally.add(r);
                    }
                    if let Some(limits) = ev.limits {
                        if newest.as_ref().is_none_or(|(at, _)| ev.at >= *at) {
                            newest = Some((ev.at, limits));
                        }
                    }
                }
            });
        if !found {
            return AgentUsage::unavailable("codex", "no local logs found");
        }
        match newest {
            Some((_, limits)) => tally.finish("codex", None, limits),
            None => tally.finish(
                "codex",
                Some("no plan limits in recent local logs".into()),
                vec![],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::clock::fakes::FixedClock;
    use std::collections::HashMap;

    // 2026-11-08T12:00:00Z
    const NOW: u64 = 1_794_139_200;

    #[derive(Default)]
    struct FakeLogs {
        files: HashMap<&'static str, Vec<Vec<String>>>,
        since: std::cell::Cell<u64>,
    }

    impl UsageLogs for FakeLogs {
        fn scan(&self, agent: LogAgent, since: u64, visit: &mut dyn FnMut(LogLine<'_>)) -> bool {
            self.since.set(since);
            let key = if agent == LogAgent::Claude {
                "claude"
            } else {
                "codex"
            };
            let Some(files) = self.files.get(key) else {
                return false;
            };
            for f in files {
                visit(LogLine::File);
                for l in f {
                    visit(LogLine::Line(l));
                }
            }
            true
        }
    }

    fn count(ts: &str, total: u64, used: f64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}}},"rate_limits":{{"primary":{{"used_percent":{used},"window_minutes":300,"resets_at":1794150000}}}}}}}}"#
        )
    }

    fn report(logs: &FakeLogs, days: i64) -> UsageReport {
        ReportUsage {
            logs,
            clock: &FixedClock::at(NOW),
            utc_offset: 0,
        }
        .run(Days::new(days).unwrap())
    }

    #[test]
    fn every_supported_agent_is_listed_and_missing_logs_are_unavailable() {
        let r = report(&FakeLogs::default(), 7);
        assert_eq!((r.days, r.generated_at), (7, NOW));
        let names: Vec<_> = r.agents.iter().map(|a| a.agent.as_str()).collect();
        assert_eq!(
            names,
            ["claude", "codex", "gemini", "cursor", "copilot", "kiro", "opencode", "aider"]
        );
        assert!(r.agents.iter().all(|a| !a.available && a.note.is_some()));
        assert_eq!(
            r.agents[2].note.as_deref(),
            Some("usage not recorded locally")
        );
    }

    #[test]
    fn only_files_modified_inside_the_period_are_asked_for() {
        let logs = FakeLogs::default();
        report(&logs, 3);
        assert_eq!(logs.since.get(), NOW - 12 * 3600 - 2 * 86_400);
    }

    #[test]
    fn codex_limits_come_from_the_newest_event_across_files() {
        let mut logs = FakeLogs::default();
        logs.files.insert(
            "codex",
            vec![
                vec![count("2026-11-08T11:00:00Z", 10, 60.0)],
                vec![count("2026-11-08T10:00:00Z", 10, 20.0)],
            ],
        );
        logs.files.insert("claude", vec![]);
        let r = report(&logs, 7);
        let codex = &r.agents[1];
        assert!(codex.available);
        assert_eq!(codex.note, None);
        assert_eq!(codex.limits[0].used_percent, 60.0);
        // Same total in two files: each file starts over, both count.
        assert_eq!(codex.period.total(), 24);
        let claude = &r.agents[0];
        assert!(claude.available && claude.limits.is_empty());
        assert_eq!(
            claude.note.as_deref(),
            Some("plan limits are not recorded locally")
        );
    }
}
