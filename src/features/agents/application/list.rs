//! `agents list` / `sessions list` across every enabled runtime.

use serde::Serialize;

use crate::features::agents::domain::{Agent, AgentError, AgentRuntime, Runtime, Session};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AgentsReport {
    pub agents: Vec<Agent>,
    /// Runtimes that are enabled but not installed or not responding.
    pub unavailable: Vec<Runtime>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SessionsReport {
    pub sessions: Vec<Session>,
    pub unavailable: Vec<Runtime>,
}

pub struct ListAgents<'a> {
    pub runtimes: &'a [&'a dyn AgentRuntime],
}

impl ListAgents<'_> {
    pub fn list(&self) -> AgentsReport {
        let mut agents = Vec::new();
        let mut unavailable = Vec::new();
        for r in self.runtimes {
            match r.list_agents() {
                Ok(a) => agents.extend(a),
                Err(_) => unavailable.push(r.runtime()),
            }
        }
        AgentsReport {
            agents,
            unavailable,
        }
    }
}

pub struct ListSessions<'a> {
    pub runtimes: &'a [&'a dyn AgentRuntime],
}

impl ListSessions<'_> {
    pub fn list(&self) -> SessionsReport {
        let mut sessions = Vec::new();
        let mut unavailable = Vec::new();
        for r in self.runtimes {
            match r.list_sessions() {
                Ok(s) => sessions.extend(s),
                Err(AgentError::Unavailable(_)) | Err(_) => unavailable.push(r.runtime()),
            }
        }
        SessionsReport {
            sessions,
            unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agents::domain::fakes::FakeRuntime;

    #[test]
    fn merges_runtimes_and_reports_unavailable() {
        let herdr = FakeRuntime::herdr_with_agents(1);
        let tmux = FakeRuntime::unavailable(Runtime::Tmux);
        let rts: [&dyn AgentRuntime; 2] = [&herdr, &tmux];
        let r = ListAgents { runtimes: &rts }.list();
        assert_eq!(r.agents.len(), 1);
        assert_eq!(r.unavailable, vec![Runtime::Tmux]);
        let s = ListSessions { runtimes: &rts }.list();
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.unavailable, vec![Runtime::Tmux]);
    }
}
