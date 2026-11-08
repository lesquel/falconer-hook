//! Agents, sessions, state-transition detection and the `AgentRuntime` port.

use serde::Serialize;
use std::collections::HashMap;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;
pub use crate::shared::domain::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "idle" => AgentStatus::Idle,
            "working" => AgentStatus::Working,
            "blocked" => AgentStatus::Blocked,
            "done" => AgentStatus::Done,
            _ => AgentStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub pane_id: String,
    pub agent: String,
    pub status: AgentStatus,
    pub state_change_seq: u64,
    pub cwd: String,
    pub title: String,
    pub workspace_id: Option<String>,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub name: String,
    pub runtime: Runtime,
    pub running: bool,
    pub default: bool,
    pub attached: Option<bool>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AgentError {
    #[error("{0} is not installed")]
    Unavailable(&'static str),
    #[error("{0} command failed")]
    Failed(&'static str),
    #[error("unexpected output from {0}")]
    Parse(&'static str),
}

impl ErrorCode for AgentError {
    fn code(&self) -> &'static str {
        match self {
            AgentError::Unavailable(_) => "runtime_unavailable",
            AgentError::Failed(_) | AgentError::Parse(_) => "runtime_error",
        }
    }
}

/// A terminal multiplexer that hosts agents. Every call runs via argv.
pub trait AgentRuntime {
    fn runtime(&self) -> Runtime;
    fn list_agents(&self) -> Result<Vec<Agent>, AgentError>;
    fn list_sessions(&self) -> Result<Vec<Session>, AgentError>;
    /// Sends named keys (`enter`, `esc`, `1`, …) to a pane.
    fn send_keys(&self, pane: &str, keys: &[String]) -> Result<(), AgentError>;
    /// Types literal text into a pane, passed as one argv entry.
    fn send_text(&self, pane: &str, text: &str) -> Result<(), AgentError>;
}

/// What a state change means for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// Entered `blocked`: the agent waits for an approval.
    Blocked(Agent),
    /// Entered `done`.
    Done(Agent),
    /// Left `blocked` without an answer from the phone (handled on the host).
    Unblocked(Agent),
}

/// Detects transitions between successive `list_agents` snapshots. The first
/// snapshot only primes the detector so a daemon restart does not re-notify.
#[derive(Debug, Default)]
pub struct TransitionDetector {
    seen: HashMap<String, (AgentStatus, u64)>,
    primed: bool,
}

impl TransitionDetector {
    pub fn observe(&mut self, agents: &[Agent]) -> Vec<Transition> {
        let mut out = Vec::new();
        let mut next = HashMap::with_capacity(agents.len());
        for a in agents {
            let prev = self.seen.get(&a.pane_id).copied();
            next.insert(a.pane_id.clone(), (a.status, a.state_change_seq));
            if !self.primed {
                continue;
            }
            let changed = match prev {
                None => true,
                Some((s, seq)) => s != a.status || seq != a.state_change_seq,
            };
            if !changed {
                continue;
            }
            match a.status {
                AgentStatus::Blocked => out.push(Transition::Blocked(a.clone())),
                AgentStatus::Done => out.push(Transition::Done(a.clone())),
                _ if matches!(prev, Some((AgentStatus::Blocked, _))) => {
                    out.push(Transition::Unblocked(a.clone()))
                }
                _ => {}
            }
        }
        self.seen = next;
        self.primed = true;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pane: &str, status: AgentStatus, seq: u64) -> Agent {
        Agent {
            pane_id: pane.into(),
            agent: "claude".into(),
            status,
            state_change_seq: seq,
            cwd: "/w".into(),
            title: "t".into(),
            workspace_id: None,
            runtime: Runtime::Herdr,
        }
    }

    #[test]
    fn first_snapshot_primes_without_events() {
        let mut d = TransitionDetector::default();
        assert!(d.observe(&[agent("p", AgentStatus::Blocked, 1)]).is_empty());
    }

    #[test]
    fn detects_blocked_done_and_unblocked() {
        use AgentStatus::*;
        let mut d = TransitionDetector::default();
        d.observe(&[agent("p", Working, 1), agent("q", Working, 1)]);
        let t = d.observe(&[agent("p", Blocked, 2), agent("q", Working, 1)]);
        assert_eq!(t, vec![Transition::Blocked(agent("p", Blocked, 2))]);
        assert!(
            d.observe(&[agent("p", Blocked, 2)]).is_empty(),
            "no repeat while unchanged"
        );
        let t = d.observe(&[agent("p", Working, 3)]);
        assert_eq!(t, vec![Transition::Unblocked(agent("p", Working, 3))]);
        let t = d.observe(&[agent("p", Done, 4)]);
        assert_eq!(t, vec![Transition::Done(agent("p", Done, 4))]);
    }

    #[test]
    fn blocked_again_between_polls_is_detected_by_seq() {
        use AgentStatus::*;
        let mut d = TransitionDetector::default();
        d.observe(&[agent("p", Blocked, 5)]);
        let t = d.observe(&[agent("p", Blocked, 7)]);
        assert_eq!(t, vec![Transition::Blocked(agent("p", Blocked, 7))]);
    }

    #[test]
    fn new_pane_after_priming_counts() {
        use AgentStatus::*;
        let mut d = TransitionDetector::default();
        d.observe(&[]);
        assert_eq!(d.observe(&[agent("n", Done, 1)]).len(), 1);
        assert!(d.observe(&[agent("n", Idle, 2)]).is_empty());
    }
}

#[cfg(test)]
pub mod fakes {
    use super::*;
    use std::cell::RefCell;

    /// Fake runtime: scripted agents, records sent keys/text, never touches a terminal.
    pub struct FakeRuntime {
        pub kind: Runtime,
        pub agents: RefCell<Vec<Agent>>,
        pub available: bool,
        pub sent: RefCell<Vec<(String, Vec<String>)>>,
        pub typed: RefCell<Vec<(String, String)>>,
    }

    impl FakeRuntime {
        pub fn herdr_with_agents(n: usize) -> Self {
            let agents = (0..n)
                .map(|i| Agent {
                    pane_id: format!("w1:p{i}"),
                    agent: "claude".into(),
                    status: AgentStatus::Working,
                    state_change_seq: 1,
                    cwd: "/w".into(),
                    title: "t".into(),
                    workspace_id: Some("w1".into()),
                    runtime: Runtime::Herdr,
                })
                .collect();
            Self {
                kind: Runtime::Herdr,
                agents: RefCell::new(agents),
                available: true,
                sent: RefCell::default(),
                typed: RefCell::default(),
            }
        }
        pub fn unavailable(kind: Runtime) -> Self {
            Self {
                kind,
                agents: RefCell::default(),
                available: false,
                sent: RefCell::default(),
                typed: RefCell::default(),
            }
        }
        pub fn set_status(&self, i: usize, s: AgentStatus, seq: u64) {
            let mut a = self.agents.borrow_mut();
            a[i].status = s;
            a[i].state_change_seq = seq;
        }
    }

    impl AgentRuntime for FakeRuntime {
        fn runtime(&self) -> Runtime {
            self.kind
        }
        fn list_agents(&self) -> Result<Vec<Agent>, AgentError> {
            if !self.available {
                return Err(AgentError::Unavailable("fake"));
            }
            Ok(self.agents.borrow().clone())
        }
        fn list_sessions(&self) -> Result<Vec<Session>, AgentError> {
            if !self.available {
                return Err(AgentError::Unavailable("fake"));
            }
            Ok(vec![Session {
                name: "default".into(),
                runtime: self.kind,
                running: true,
                default: true,
                attached: None,
            }])
        }
        fn send_keys(&self, pane: &str, keys: &[String]) -> Result<(), AgentError> {
            if !self.available {
                return Err(AgentError::Unavailable("fake"));
            }
            self.sent.borrow_mut().push((pane.into(), keys.to_vec()));
            Ok(())
        }
        fn send_text(&self, pane: &str, text: &str) -> Result<(), AgentError> {
            if !self.available {
                return Err(AgentError::Unavailable("fake"));
            }
            self.typed.borrow_mut().push((pane.into(), text.into()));
            Ok(())
        }
    }
}
