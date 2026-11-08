//! Herdr adapter (verified against herdr 0.9.0 output).
//!
//! - `herdr agent list` → `{"result":{"agents":[{pane_id, agent, agent_status, state_change_seq, cwd, terminal_title_stripped, workspace_id}]}}`
//! - `herdr session list --json` → `{"sessions":[{name, running, default}]}`
//! - keys: `herdr agent send-keys <pane> <key>...`
//! - text: `herdr pane send-text <pane> <text>` (one argv entry)

use serde::Deserialize;

use super::run;
use crate::features::agents::domain::{
    Agent, AgentError, AgentRuntime, AgentStatus, Runtime, Session,
};
use crate::features::events::domain::valid_pane_id;
use crate::shared::domain::command::CommandRunner;

pub struct HerdrRuntime<'a> {
    pub runner: &'a dyn CommandRunner,
    pub program: String,
}

#[derive(Deserialize)]
struct AgentList {
    result: AgentListResult,
}
#[derive(Deserialize)]
struct AgentListResult {
    agents: Vec<RawAgent>,
}
#[derive(Deserialize)]
struct RawAgent {
    pane_id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    agent_status: Option<String>,
    #[serde(default)]
    state_change_seq: u64,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    terminal_title_stripped: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
}
#[derive(Deserialize)]
struct SessionList {
    sessions: Vec<RawSession>,
}
#[derive(Deserialize)]
struct RawSession {
    name: String,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    default: bool,
}

pub fn parse_agents(stdout: &str) -> Result<Vec<Agent>, AgentError> {
    let list: AgentList = serde_json::from_str(stdout).map_err(|_| AgentError::Parse("herdr"))?;
    Ok(list
        .result
        .agents
        .into_iter()
        .filter(|a| valid_pane_id(&a.pane_id))
        .map(|a| Agent {
            pane_id: a.pane_id,
            agent: a.agent.unwrap_or_else(|| "unknown".into()),
            status: AgentStatus::parse(a.agent_status.as_deref().unwrap_or("unknown")),
            state_change_seq: a.state_change_seq,
            cwd: a.cwd.unwrap_or_default(),
            title: a.terminal_title_stripped.unwrap_or_default(),
            workspace_id: a.workspace_id,
            runtime: Runtime::Herdr,
        })
        .collect())
}

pub fn parse_sessions(stdout: &str) -> Result<Vec<Session>, AgentError> {
    let list: SessionList = serde_json::from_str(stdout).map_err(|_| AgentError::Parse("herdr"))?;
    Ok(list
        .sessions
        .into_iter()
        .map(|s| Session {
            name: s.name,
            runtime: Runtime::Herdr,
            running: s.running,
            default: s.default,
            attached: None,
        })
        .collect())
}

impl HerdrRuntime<'_> {
    fn ok(&self, args: &[&str]) -> Result<String, AgentError> {
        let out = run(self.runner, "herdr", &self.program, args)?;
        if !out.success() {
            return Err(AgentError::Failed("herdr"));
        }
        Ok(out.stdout)
    }
}

impl AgentRuntime for HerdrRuntime<'_> {
    fn runtime(&self) -> Runtime {
        Runtime::Herdr
    }

    fn list_agents(&self) -> Result<Vec<Agent>, AgentError> {
        parse_agents(&self.ok(&["agent", "list"])?)
    }

    fn list_sessions(&self) -> Result<Vec<Session>, AgentError> {
        parse_sessions(&self.ok(&["session", "list", "--json"])?)
    }

    fn send_keys(&self, pane: &str, keys: &[String]) -> Result<(), AgentError> {
        if !valid_pane_id(pane) || keys.is_empty() {
            return Err(AgentError::Failed("herdr"));
        }
        let mut args = vec!["agent", "send-keys", pane];
        args.extend(keys.iter().map(String::as_str));
        self.ok(&args).map(|_| ())
    }

    fn send_text(&self, pane: &str, text: &str) -> Result<(), AgentError> {
        if !valid_pane_id(pane) {
            return Err(AgentError::Failed("herdr"));
        }
        // `--` only when the text could be mistaken for an option.
        if text.starts_with('-') {
            self.ok(&["pane", "send-text", "--", pane, text])
                .map(|_| ())
        } else {
            self.ok(&["pane", "send-text", pane, text]).map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;
    use crate::shared::domain::command::{CommandError, CommandOutput};

    const LIST: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_session":{"agent":"claude"},"agent_status":"blocked","cwd":"/home/u","focused":false,"pane_id":"w7:pA","revision":2,"state_change_seq":21,"tab_id":"w7:tA","terminal_title":"x","terminal_title_stripped":"Fix tests","workspace_id":"w7"},{"pane_id":"bad pane","agent_status":"idle"}],"type":"agent_list"}}"#;

    fn rt(r: &RecordingRunner) -> HerdrRuntime<'_> {
        HerdrRuntime {
            runner: r,
            program: "herdr".into(),
        }
    }

    #[test]
    fn parses_real_agent_list_shape() {
        let r = RecordingRunner::default();
        r.reply_ok(LIST);
        let agents = rt(&r).list_agents().unwrap();
        assert_eq!(r.argv(), vec![vec!["herdr", "agent", "list"]]);
        assert_eq!(agents.len(), 1, "invalid pane ids are dropped");
        let a = &agents[0];
        assert_eq!(
            (a.pane_id.as_str(), a.status, a.state_change_seq),
            ("w7:pA", AgentStatus::Blocked, 21)
        );
        assert_eq!(a.title, "Fix tests");
        assert_eq!(a.workspace_id.as_deref(), Some("w7"));
    }

    #[test]
    fn parses_sessions() {
        let r = RecordingRunner::default();
        r.reply_ok(r#"{"sessions":[{"default":true,"name":"default","running":true}]}"#);
        let s = rt(&r).list_sessions().unwrap();
        assert_eq!(r.argv(), vec![vec!["herdr", "session", "list", "--json"]]);
        assert_eq!(s[0].name, "default");
        assert!(s[0].running && s[0].default);
    }

    #[test]
    fn send_keys_and_text_argv() {
        let r = RecordingRunner::default();
        let h = rt(&r);
        h.send_keys("w7:pA", &["enter".into()]).unwrap();
        h.send_text("w7:pA", "use $(rm -rf ~); `x`\nok").unwrap();
        h.send_text("w7:pA", "-rf please").unwrap();
        assert_eq!(
            r.argv(),
            vec![
                vec!["herdr", "agent", "send-keys", "w7:pA", "enter"],
                vec![
                    "herdr",
                    "pane",
                    "send-text",
                    "w7:pA",
                    "use $(rm -rf ~); `x`\nok"
                ],
                vec!["herdr", "pane", "send-text", "--", "w7:pA", "-rf please"],
            ]
        );
    }

    #[test]
    fn errors_map_to_domain() {
        let r = RecordingRunner::default();
        r.reply(Err(CommandError::NotFound("herdr".into())));
        assert_eq!(rt(&r).list_agents(), Err(AgentError::Unavailable("herdr")));
        r.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "boom".into(),
        }));
        assert_eq!(rt(&r).list_agents(), Err(AgentError::Failed("herdr")));
        r.reply_ok("garbage");
        assert_eq!(rt(&r).list_agents(), Err(AgentError::Parse("herdr")));
        assert_eq!(
            rt(&r).send_keys("-t", &["enter".into()]),
            Err(AgentError::Failed("herdr"))
        );
    }
}
