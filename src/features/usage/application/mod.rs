//! Use case: the usage report over the agents' local logs.

use crate::features::usage::domain::{
    AgentUsage, CodexParser, CodexSession, Days, Limit, Tally, UsageReport, Window,
};
use crate::shared::domain::clock::Clock;

/// What a scan hands to its visitor, file by file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLine<'a> {
    /// A new file starts; the lines that follow belong to it.
    File,
    Line(&'a str),
}

/// Port over Codex's session rollouts (read-only, bounded).
pub trait UsageLogs {
    /// Visits the newest rollout files modified at or after `modified_since`,
    /// each as `File` followed by the lines of its head and its tail only.
    /// Returns `false` when Codex has no sessions directory.
    fn codex(&self, modified_since: u64, visit: &mut dyn FnMut(LogLine<'_>)) -> bool;
}

/// Claude Code is never read: its usage lives only inside its transcripts.
pub const CLAUDE_NOTE: &str =
    "not read: Claude Code keeps no usage summary, and its transcripts are too large to scan";

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
        let mut agents = vec![
            AgentUsage::unavailable("claude", CLAUDE_NOTE),
            self.codex(window),
        ];
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

    /// One record per session, at its last event: a session that spans
    /// midnight counts on the day it ended.
    fn codex(&self, window: Window) -> AgentUsage {
        let mut parser = CodexParser::default();
        let mut sessions: Vec<CodexSession> = Vec::new();
        let found = self.logs.codex(window.period_start, &mut |l| match l {
            LogLine::File => sessions.push(parser.take()),
            LogLine::Line(line) => parser.feed(line),
        });
        sessions.push(parser.take());
        let mut tally = Tally::new(window);
        let mut newest: Option<(u64, Vec<Limit>)> = None;
        for s in sessions {
            if let Some(r) = s.record {
                tally.add(r);
            }
            if let Some((at, limits)) = s.limits {
                if newest.as_ref().is_none_or(|(t, _)| at >= *t) {
                    newest = Some((at, limits));
                }
            }
        }
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
        fn codex(&self, since: u64, visit: &mut dyn FnMut(LogLine<'_>)) -> bool {
            self.since.set(since);
            let Some(files) = self.files.get("codex") else {
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

    fn count(ts: &str, input: u64, used: f64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":0}}}},"rate_limits":{{"primary":{{"used_percent":{used},"window_minutes":300,"resets_at":1794150000}}}}}}}}"#
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
    fn claude_is_never_scanned_and_reported_unavailable() {
        let mut logs = FakeLogs::default();
        logs.files.insert("codex", vec![]);
        let r = report(&logs, 7);
        assert_eq!(r.agents[0], AgentUsage::unavailable("claude", CLAUDE_NOTE));
        assert!(r.agents[1].available);
    }

    #[test]
    fn codex_counts_each_session_once_and_limits_come_from_the_newest_file() {
        let mut logs = FakeLogs::default();
        logs.files.insert(
            "codex",
            vec![
                vec![
                    count("2026-11-08T10:30:00Z", 5, 10.0),
                    count("2026-11-08T11:00:00Z", 10, 60.0),
                ],
                vec![count("2026-11-08T10:00:00Z", 7, 20.0)],
                // Ended before the period: not counted, its limits are older.
                vec![count("2026-10-01T10:00:00Z", 1000, 90.0)],
            ],
        );
        let r = report(&logs, 7);
        let codex = &r.agents[1];
        assert!(codex.available);
        assert_eq!(codex.note, None);
        assert_eq!(codex.limits[0].used_percent, 60.0);
        assert_eq!(codex.period.total(), 17);
        assert_eq!(codex.today.total(), 17);
    }

    #[test]
    fn codex_without_limits_says_so() {
        let mut logs = FakeLogs::default();
        logs.files.insert("codex", vec![vec!["{}".into()]]);
        let codex = &report(&logs, 7).agents[1];
        assert!(codex.available && codex.limits.is_empty());
        assert_eq!(
            codex.note.as_deref(),
            Some("no plan limits in recent local logs")
        );
    }
}
