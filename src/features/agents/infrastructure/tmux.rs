//! tmux adapter. tmux has no agent model: agents managed only by tmux report
//! through their own hooks (`falconer-hook emit`), so `list_agents` is empty.
//!
//! - sessions: `tmux list-sessions -F '#{session_name}\t#{session_attached}'`
//! - keys: `tmux send-keys -t <pane> <Key>...` (names mapped to tmux names)
//! - text: `tmux send-keys -t <pane> -l -- <text>` (literal, one argv entry)
//! - probe: `tmux display-message -p -t <pane> '#{pane_pid}'`
//! - a pane recorded with a server socket gets `tmux -S <socket> …`

use super::run;
use crate::features::agents::domain::{
    Agent, AgentError, AgentRuntime, PaneProbe, PaneRef, PaneState, Runtime, Session,
};
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

impl TmuxRuntime<'_> {
    /// Runs `args` against the pane's server (`-S <socket>` first).
    fn on(
        &self,
        pane: &PaneRef,
        args: &[&str],
    ) -> Result<crate::shared::domain::command::CommandOutput, AgentError> {
        let mut argv: Vec<&str> = Vec::with_capacity(args.len() + 2);
        if let Some(s) = pane.session() {
            argv.extend(["-S", s]);
        }
        argv.extend(args);
        run(self.runner, "tmux", &self.program, &argv)
    }
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

    fn send_keys(&self, pane: &PaneRef, keys: &[String]) -> Result<(), AgentError> {
        let id = pane.pane.as_str();
        if !valid_pane_id(id) || keys.is_empty() {
            return Err(AgentError::Failed("tmux"));
        }
        let mapped: Option<Vec<&str>> = keys.iter().map(|k| tmux_key(k)).collect();
        let mapped = mapped.ok_or(AgentError::Failed("tmux"))?;
        let mut args = vec!["send-keys", "-t", id];
        args.extend(mapped);
        let out = self.on(pane, &args)?;
        out.success()
            .then_some(())
            .ok_or(AgentError::Failed("tmux"))
    }

    fn send_text(&self, pane: &PaneRef, text: &str) -> Result<(), AgentError> {
        let id = pane.pane.as_str();
        if !valid_pane_id(id) {
            return Err(AgentError::Failed("tmux"));
        }
        let out = self.on(pane, &["send-keys", "-t", id, "-l", "--", text])?;
        out.success()
            .then_some(())
            .ok_or(AgentError::Failed("tmux"))
    }

    fn probe(&self, pane: &PaneRef) -> Result<PaneProbe, AgentError> {
        if !valid_pane_id(&pane.pane) {
            return Ok(PaneProbe::Gone);
        }
        let out = self.on(
            pane,
            &["display-message", "-p", "-t", &pane.pane, "#{pane_pid}"],
        )?;
        // A missing pane or server makes tmux fail: either way it is gone.
        if !out.success() {
            return Ok(PaneProbe::Gone);
        }
        let pid = out
            .stdout
            .trim()
            .parse::<u32>()
            .map_err(|_| AgentError::Parse("tmux"))?;
        Ok(PaneProbe::Live(PaneState {
            pane_pid: Some(pid),
            ..PaneState::default()
        }))
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

    fn pane(id: &str) -> PaneRef {
        PaneRef::new(Runtime::Tmux, id)
    }

    #[test]
    fn send_argv_is_exact() {
        let r = RecordingRunner::default();
        let t = rt(&r);
        t.send_keys(&pane("%3"), &["esc".into()]).unwrap();
        t.send_keys(&pane("%3"), &["1".into(), "enter".into()])
            .unwrap();
        t.send_text(&pane("%3"), "-x; $(id)").unwrap();
        let mut sock = pane("%3");
        sock.context.session = Some("/tmp/tmux-1000/work".into());
        t.send_keys(&sock, &["enter".into()]).unwrap();
        assert_eq!(
            r.argv(),
            vec![
                vec!["tmux", "send-keys", "-t", "%3", "Escape"],
                vec!["tmux", "send-keys", "-t", "%3", "1", "Enter"],
                vec!["tmux", "send-keys", "-t", "%3", "-l", "--", "-x; $(id)"],
                vec![
                    "tmux",
                    "-S",
                    "/tmp/tmux-1000/work",
                    "send-keys",
                    "-t",
                    "%3",
                    "Enter"
                ],
            ]
        );
    }

    #[test]
    fn probe_reads_the_pane_pid_on_its_socket() {
        let r = RecordingRunner::default();
        r.reply_ok("4242\n");
        r.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "can't find pane: %9".into(),
        }));
        let mut sock = pane("%3");
        sock.context.session = Some("/tmp/tmux-1000/default".into());
        assert_eq!(
            rt(&r).probe(&sock).unwrap(),
            PaneProbe::Live(PaneState {
                pane_pid: Some(4242),
                ..PaneState::default()
            })
        );
        assert_eq!(rt(&r).probe(&pane("%9")).unwrap(), PaneProbe::Gone);
        assert_eq!(
            r.argv()[0],
            vec![
                "tmux",
                "-S",
                "/tmp/tmux-1000/default",
                "display-message",
                "-p",
                "-t",
                "%3",
                "#{pane_pid}"
            ]
        );
    }

    #[test]
    fn unknown_key_names_are_refused() {
        let r = RecordingRunner::default();
        assert_eq!(
            rt(&r).send_keys(&pane("%3"), &["C-c".into()]),
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
