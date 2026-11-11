//! Navigator use cases: long-polled snapshot and pane screen, and the
//! allowlisted actions.

use serde::Serialize;
use std::time::Duration;

use crate::features::mirror::domain::MonoClock;
use crate::features::navigator::domain::{
    check_pane, rev_of, screen_lines, validate, NavError, NavSnapshot, NavigatorApi, Op,
    PaneScreen, Polled, MAX_WAIT_MS, POLL_MS,
};
use crate::features::responses::domain::sanitize_text;

/// Long-poll parameters: the revision the caller has and how long it may wait.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Poll {
    pub since: Option<String>,
    pub wait_ms: u64,
}

/// Fetches until the revision differs from `since` or the wait (capped at
/// [`MAX_WAIT_MS`]) is over, re-reading every [`POLL_MS`]. Without `since`
/// the first read is returned.
pub fn long_poll<T: Serialize, C: MonoClock>(
    clock: &C,
    poll: &Poll,
    mut fetch: impl FnMut() -> Result<T, NavError>,
) -> Result<Polled<T>, NavError> {
    let deadline = clock.now_ms() + poll.wait_ms.min(MAX_WAIT_MS);
    loop {
        let body = fetch()?;
        let rev = rev_of(&body);
        if poll.since.as_deref() != Some(rev.as_str()) {
            return Ok(Polled {
                rev,
                unchanged: false,
                body: Some(body),
            });
        }
        let now = clock.now_ms();
        if now >= deadline {
            return Ok(Polled {
                rev,
                unchanged: true,
                body: None,
            });
        }
        clock.sleep(Duration::from_millis(POLL_MS.min(deadline - now)));
    }
}

/// `herdr snapshot`.
pub fn snapshot<A: NavigatorApi, C: MonoClock>(
    api: &mut A,
    clock: &C,
    poll: &Poll,
) -> Result<Polled<NavSnapshot>, NavError> {
    long_poll(clock, poll, || api.snapshot())
}

/// `herdr read`: the pane's last `lines` lines with its agent and status.
pub fn read_screen<A: NavigatorApi, C: MonoClock>(
    api: &mut A,
    clock: &C,
    poll: &Poll,
    pane_id: &str,
    lines: u32,
) -> Result<Polled<PaneScreen>, NavError> {
    long_poll(clock, poll, || {
        let snap = api.snapshot()?;
        check_pane(&snap, pane_id)?;
        let pane = snap
            .pane(pane_id)
            .cloned()
            .ok_or(NavError::Protocol("pane"))?;
        let text = api.read_pane(pane_id, lines)?;
        Ok(PaneScreen {
            agent: pane.agent,
            agent_status: pane.agent_status,
            lines: screen_lines(&text, lines as usize),
        })
    })
}

/// What `herdr action` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionDone {
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Validates `op` against a fresh snapshot, then runs it. Text is sanitized
/// (control characters removed) and sent over the socket like every other
/// action, never through the `herdr` binary, which an SSH session may not
/// have on PATH; then Enter when `submit` is set.
pub fn execute<A: NavigatorApi>(api: &mut A, op: &Op) -> Result<ActionDone, NavError> {
    let snap = api.snapshot()?;
    validate(op, &snap)?;
    let id = match op {
        Op::PaneText(t) => {
            let clean = sanitize_text(&t.text).map_err(|e| NavError::Invalid(e.to_string()))?;
            api.apply(&Op::PaneText(crate::features::navigator::domain::Text {
                text: clean,
                ..t.clone()
            }))?;
            if t.submit {
                api.apply(&Op::PaneKeys(crate::features::navigator::domain::Keys {
                    pane_id: t.pane_id.clone(),
                    keys: vec!["enter".into()],
                }))?;
            }
            None
        }
        other => api.apply(other)?,
    };
    Ok(ActionDone { op: op.name(), id })
}

#[cfg(test)]
mod tests;
