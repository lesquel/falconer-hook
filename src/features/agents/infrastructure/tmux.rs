//! tmux adapter. tmux has no agent model: agents managed only by tmux report
//! through their own hooks (`openmosh-hook emit`), so `list_agents` is empty.
//!
//! - sessions: `tmux list-sessions -F '#{session_name}\t#{session_attached}'`
//! - keys: `tmux send-keys -t <pane> <Key>...` (names mapped to tmux names)
//! - text: `tmux send-keys -t <pane> -l -- <text>` (literal, one argv entry)

use super::run;
use crate::features::agents::domain::{Agent, AgentError, AgentRuntime, Runtime, Session};
use crate::features::events::domain::valid_pane_id;
use crate::shared::domain::command::CommandRunner;

pub struct TmuxRuntime<'a> {
    pub runner: &'a dyn CommandRunner,
    pub program: String,
}

/// Maps keymap key names to tmux key names. Unknown multi-char names are refused.
pub fn tmux_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "enter" => "Enter",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "space" => "Space",
        "backspace" => "BSpace",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "y" => "y",
        "n" => "n",
        "1" => "1",
        "2" => "2",
        "3" => "3",
        _ => return None,
    })
}

pub fn parse_sessions(stdout: &str) -> Vec<Session> {
    stdout
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let name = it.next()?.to_string();
            if name.is_empty() {
                return None;
            }
            let attached = it.next().map(|a| a.trim() != "0");
            Some(Session {
                name,
                runtime: Runtime::Tmux,
                running: true,
                default: false,
                attached,
            })
        })
        .collect()
}

impl AgentRuntime for TmuxRuntime<'_> {
    fn runtime(&self) -> Runtime {
        Runtime::Tmux
    }

    fn list_agents(&self) -> Result<Vec<Agent>, AgentError> {
        Ok(Vec::new())
    }

    fn list_sessions(&self) -> Result<Vec<Session>, AgentError> {
        let out = run(
            self.runner,
            "tmux",
            &self.program,
            &[
                "list-sessions",
                "-F",
                "#{session_name}\t#{session_attached}",
            ],
        )?;
        if !out.success() {
            // No server running means no sessions, not an error.
            if out.stderr.contains("no server running") || out.stderr.contains("No such file") {
                return Ok(Vec::new());
            }
            return Err(AgentError::Failed("tmux"));
        }
        Ok(parse_sessions(&out.stdout))
    }

    fn send_keys(&self, pane: &str, keys: &[String]) -> Result<(), AgentError> {
        if !valid_pane_id(pane) || keys.is_empty() {
            return Err(AgentError::Failed("tmux"));
        }
        let mapped: Option<Vec<&str>> = keys.iter().map(|k| tmux_key(k)).collect();
        let mapped = mapped.ok_or(AgentError::Failed("tmux"))?;
        let mut args = vec!["send-keys", "-t", pane];
        args.extend(mapped);
        let out = run(self.runner, "tmux", &self.program, &args)?;
        out.success()
            .then_some(())
            .ok_or(AgentError::Failed("tmux"))
    }

    fn send_text(&self, pane: &str, text: &str) -> Result<(), AgentError> {
        if !valid_pane_id(pane) {
            return Err(AgentError::Failed("tmux"));
        }
        let out = run(
            self.runner,
            "tmux",
            &self.program,
            &["send-keys", "-t", pane, "-l", "--", text],
        )?;
        out.success()
            .then_some(())
            .ok_or(AgentError::Failed("tmux"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;
    use crate::shared::domain::command::CommandOutput;

    fn rt(r: &RecordingRunner) -> TmuxRuntime<'_> {
        TmuxRuntime {
            runner: r,
            program: "tmux".into(),
        }
    }

    #[test]
    fn send_argv_is_exact() {
        let r = RecordingRunner::default();
        let t = rt(&r);
        t.send_keys("%3", &["esc".into()]).unwrap();
        t.send_keys("%3", &["1".into(), "enter".into()]).unwrap();
        t.send_text("%3", "-x; $(id)").unwrap();
        assert_eq!(
            r.argv(),
            vec![
                vec!["tmux", "send-keys", "-t", "%3", "Escape"],
                vec!["tmux", "send-keys", "-t", "%3", "1", "Enter"],
                vec!["tmux", "send-keys", "-t", "%3", "-l", "--", "-x; $(id)"],
            ]
        );
    }

    #[test]
    fn unknown_key_names_are_refused() {
        let r = RecordingRunner::default();
        assert_eq!(
            rt(&r).send_keys("%3", &["C-c".into()]),
            Err(AgentError::Failed("tmux"))
        );
        assert!(r.argv().is_empty());
    }

    #[test]
    fn sessions_and_no_server() {
        let r = RecordingRunner::default();
        r.reply_ok("main\t1\nwork\t0\n");
        let s = rt(&r).list_sessions().unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].attached, Some(true));
        assert_eq!(s[1].attached, Some(false));
        r.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "no server running on /tmp/tmux-1000/default".into(),
        }));
        assert!(rt(&r).list_sessions().unwrap().is_empty());
    }
}
