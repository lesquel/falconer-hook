//! `herdr snapshot|read|action`: the phone's Herdr navigator over the
//! session's socket (owned by the owner of `$HOME`), plus the existing
//! Herdr CLI adapter for typing text.

use std::fmt::Write as _;

use super::commands::read_stdin;
use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::cli::{HerdrCmd, PollArgs};
use crate::features::mirror::domain::valid_session_name;
use crate::features::mirror::infrastructure::{socket_path, SystemMonoClock};
use crate::features::navigator::application::{execute, read_screen, snapshot, Poll};
use crate::features::navigator::domain::{NavSnapshot, Op, MAX_LINES};
use crate::features::navigator::infrastructure::SocketNavigator;

/// Largest accepted action request (4000 characters of text, JSON-escaped).
const MAX_REQUEST_BYTES: usize = 64 * 1024;

fn invalid(msg: &str) -> CliError {
    CliError::new("invalid_request", msg)
}

fn check_session(session: Option<&str>) -> Result<(), CliError> {
    match session {
        Some(s) if !valid_session_name(s) => Err(invalid("invalid session name")),
        _ => Ok(()),
    }
}

fn navigator(ctx: &Ctx, session: Option<&str>) -> SocketNavigator {
    let path = socket_path(
        session,
        |k| std::env::var(k).ok(),
        &ctx.paths.xdg_config_home,
    );
    let owner = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&ctx.paths.home).map_or(u32::MAX, |m| m.uid())
    };
    SocketNavigator::new(path, Some(owner))
}

fn poll_of(p: &PollArgs) -> Poll {
    Poll {
        since: p.since.clone(),
        wait_ms: p.wait_ms,
    }
}

pub fn run(ctx: &Ctx, cmd: HerdrCmd) -> Result<Reply, CliError> {
    let clock = SystemMonoClock::default();
    match cmd {
        HerdrCmd::Snapshot { poll, .. } => {
            check_session(poll.session.as_deref())?;
            let mut nav = navigator(ctx, poll.session.as_deref());
            let got = snapshot(&mut nav, &clock, &poll_of(&poll))?;
            let human = got
                .body
                .as_ref()
                .map_or_else(|| format!("unchanged ({})", got.rev), human_snapshot);
            Ok(Reply::new(&got, human))
        }
        HerdrCmd::Read {
            pane, lines, poll, ..
        } => {
            check_session(poll.session.as_deref())?;
            if lines == 0 || lines > MAX_LINES {
                return Err(invalid("--lines must be 1-400"));
            }
            let mut nav = navigator(ctx, poll.session.as_deref());
            let got = read_screen(&mut nav, &clock, &poll_of(&poll), &pane, lines)?;
            let human = got.body.as_ref().map_or_else(
                || format!("unchanged ({})", got.rev),
                |b| b.lines.join("\n"),
            );
            Ok(Reply::new(&got, human))
        }
        HerdrCmd::Action { session, .. } => {
            check_session(session.as_deref())?;
            let op = Op::parse(&read_stdin(MAX_REQUEST_BYTES)?)?;
            let mut nav = navigator(ctx, session.as_deref());
            let done = execute(&mut nav, &op)?;
            let human = match &done.id {
                Some(id) => format!("{} {id}", done.op),
                None => done.op.to_string(),
            };
            Ok(Reply::new(&done, human))
        }
    }
}

fn human_snapshot(s: &NavSnapshot) -> String {
    let mut out = String::new();
    for w in &s.workspaces {
        let _ = writeln!(out, "{} {}", w.id, w.label);
        for t in s.tabs.iter().filter(|t| t.workspace_id == w.id) {
            let _ = writeln!(out, "  {} {}", t.id, t.label);
            for p in s.panes.iter().filter(|p| p.tab_id == t.id) {
                let status = serde_json::to_value(p.agent_status).unwrap_or_default();
                let _ = writeln!(
                    out,
                    "    {} {} {}",
                    p.id,
                    p.agent.as_deref().unwrap_or("-"),
                    status.as_str().unwrap_or("unknown")
                );
            }
        }
    }
    out.trim_end().to_string()
}
