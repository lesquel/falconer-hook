use super::*;
use crate::features::agents::domain::AgentStatus;
use crate::features::navigator::domain::{NavPane, NavTab, NavWorkspace};
use crate::shared::domain::command::fakes::RecordingRunner;
use crate::shared::domain::error::ErrorCode;
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct FakeClock {
    now: Cell<u64>,
    sleeps: RefCell<Vec<u64>>,
}

impl MonoClock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now.get()
    }
    fn sleep(&self, d: Duration) {
        self.sleeps.borrow_mut().push(d.as_millis() as u64);
        self.now.set(self.now.get() + d.as_millis() as u64);
    }
}

/// Serves scripted snapshots in order (the last one repeats) and records calls.
#[derive(Default)]
struct FakeApi {
    snaps: Vec<NavSnapshot>,
    reads: usize,
    text: String,
    /// Texts served by successive `read_pane` calls (the last repeats);
    /// `text` when empty.
    texts: Vec<String>,
    text_reads: usize,
    /// Markers served in order (the last repeats); none → unavailable.
    markers: Vec<String>,
    marker_calls: usize,
    calls: Vec<String>,
}

impl NavigatorApi for FakeApi {
    fn snapshot(&mut self) -> Result<NavSnapshot, NavError> {
        let i = self.reads.min(self.snaps.len() - 1);
        self.reads += 1;
        self.calls.push("snapshot".into());
        Ok(self.snaps[i].clone())
    }
    fn read_pane(&mut self, pane_id: &str, lines: u32) -> Result<String, NavError> {
        self.calls.push(format!("read {pane_id} {lines}"));
        let i = self.text_reads.min(self.texts.len().saturating_sub(1));
        self.text_reads += 1;
        Ok(self.texts.get(i).unwrap_or(&self.text).clone())
    }
    fn pane_marker(&mut self, _pane_id: &str) -> Result<String, NavError> {
        let i = self.marker_calls.min(self.markers.len().saturating_sub(1));
        self.marker_calls += 1;
        self.markers.get(i).cloned().ok_or(NavError::Unavailable)
    }
    fn apply(&mut self, op: &Op) -> Result<Option<String>, NavError> {
        self.calls.push(format!("apply {op:?}"));
        Ok(matches!(op, Op::WorkspaceCreate(_)).then(|| "w2".to_string()))
    }
}

fn snap(status: AgentStatus) -> NavSnapshot {
    NavSnapshot {
        workspaces: vec![NavWorkspace {
            id: "w1".into(),
            label: "api".into(),
            focused: true,
        }],
        tabs: vec![NavTab {
            id: "w1:t1".into(),
            workspace_id: "w1".into(),
            label: "1".into(),
            number: 1,
            agent_status: status,
            pane_count: 1,
            focused: true,
        }],
        panes: vec![NavPane {
            id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            agent: Some("claude".into()),
            agent_status: status,
            focused: true,
        }],
    }
}

fn api(snaps: Vec<NavSnapshot>) -> FakeApi {
    FakeApi {
        snaps,
        ..Default::default()
    }
}

#[test]
fn without_since_the_first_snapshot_is_returned() {
    let (mut a, clock) = (api(vec![snap(AgentStatus::Idle)]), FakeClock::default());
    let poll = Poll {
        since: None,
        wait_ms: 20_000,
    };
    let got = snapshot(&mut a, &clock, &poll).unwrap();
    assert!(!got.unchanged);
    assert_eq!(got.rev, rev_of(&snap(AgentStatus::Idle)));
    assert_eq!(got.body, Some(snap(AgentStatus::Idle)));
    assert!(clock.sleeps.borrow().is_empty());
}

#[test]
fn unchanged_without_wait_answers_at_once() {
    let rev = rev_of(&snap(AgentStatus::Idle));
    let (mut a, clock) = (api(vec![snap(AgentStatus::Idle)]), FakeClock::default());
    let poll = Poll {
        since: Some(rev.clone()),
        wait_ms: 0,
    };
    let got = snapshot(&mut a, &clock, &poll).unwrap();
    assert_eq!((got.rev, got.unchanged, got.body), (rev, true, None));
    assert_eq!(a.reads, 1);
}

#[test]
fn long_poll_returns_as_soon_as_the_state_changes() {
    let rev = rev_of(&snap(AgentStatus::Working));
    let mut a = api(vec![
        snap(AgentStatus::Working),
        snap(AgentStatus::Working),
        snap(AgentStatus::Blocked),
    ]);
    let clock = FakeClock::default();
    let poll = Poll {
        since: Some(rev),
        wait_ms: 20_000,
    };
    let got = snapshot(&mut a, &clock, &poll).unwrap();
    assert!(!got.unchanged);
    assert_eq!(got.body, Some(snap(AgentStatus::Blocked)));
    assert_eq!(*clock.sleeps.borrow(), vec![250, 250]);
}

#[test]
fn long_poll_gives_up_at_the_capped_deadline() {
    let rev = rev_of(&snap(AgentStatus::Working));
    let mut a = api(vec![snap(AgentStatus::Working)]);
    let clock = FakeClock::default();
    let poll = Poll {
        since: Some(rev.clone()),
        wait_ms: 999_999,
    };
    let got = snapshot(&mut a, &clock, &poll).unwrap();
    assert_eq!((got.rev, got.unchanged), (rev, true));
    assert_eq!(clock.now.get(), MAX_WAIT_MS, "capped at 20 s");
    assert!(clock.sleeps.borrow().iter().all(|&s| s <= POLL_MS));
    assert_eq!(a.reads as u64, MAX_WAIT_MS / POLL_MS + 1);

    // A deadline that is not a multiple of the poll interval is met exactly.
    let mut a = api(vec![snap(AgentStatus::Working)]);
    let clock = FakeClock::default();
    let poll = Poll {
        since: Some(rev_of(&snap(AgentStatus::Working))),
        wait_ms: 600,
    };
    assert!(snapshot(&mut a, &clock, &poll).unwrap().unchanged);
    assert_eq!(*clock.sleeps.borrow(), vec![250, 250, 100]);
}

#[test]
fn read_returns_clean_lines_with_the_pane_agent() {
    let mut a = api(vec![snap(AgentStatus::Blocked)]);
    a.text = "\u{1b}[1mDo you want to proceed?\u{1b}[0m\n❯ 1. Yes\n  2. No\n".into();
    let clock = FakeClock::default();
    let got = read_screen(&mut a, &clock, &Poll::default(), "w1:p1", 2).unwrap();
    let body = got.body.unwrap();
    assert_eq!(body.agent.as_deref(), Some("claude"));
    assert_eq!(body.agent_status, AgentStatus::Blocked);
    assert_eq!(body.lines, vec!["❯ 1. Yes", "  2. No"]);
    assert_eq!(a.calls, vec!["snapshot", "read w1:p1 2"]);
    assert_eq!(got.rev, rev_of(&body));

    let since = Poll {
        since: Some(got.rev.clone()),
        wait_ms: 0,
    };
    let again = read_screen(&mut a, &clock, &since, "w1:p1", 2).unwrap();
    assert!(again.unchanged && again.body.is_none());
}

fn screen(text: &str) -> PaneScreen {
    PaneScreen {
        agent: Some("claude".into()),
        agent_status: AgentStatus::Idle,
        lines: screen_lines(text, 2000),
    }
}

fn read_poll(text: &str, wait_ms: u64) -> Poll {
    Poll {
        since: Some(rev_of(&screen(text))),
        wait_ms,
    }
}

#[test]
fn an_unchanged_pane_marker_skips_the_full_read_while_waiting() {
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    a.text = "hello".into();
    a.markers = vec!["m1".into()];
    let clock = FakeClock::default();
    let got = read_screen(&mut a, &clock, &read_poll("hello", 1000), "w1:p1", 2000).unwrap();
    assert!(got.unchanged && got.body.is_none());
    assert_eq!(got.rev, rev_of(&screen("hello")));
    assert_eq!(
        a.calls,
        vec!["snapshot", "read w1:p1 2000"],
        "one full read"
    );
    assert_eq!(a.marker_calls, 5, "one cheap check per tick");
    assert_eq!(*clock.sleeps.borrow(), vec![250, 250, 250, 250]);
}

#[test]
fn a_changed_pane_marker_reads_once_and_returns_early() {
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    a.texts = vec!["old".into(), "new".into()];
    a.markers = vec!["m1".into(), "m1".into(), "m2".into()];
    let clock = FakeClock::default();
    let got = read_screen(&mut a, &clock, &read_poll("old", 20_000), "w1:p1", 2000).unwrap();
    assert!(!got.unchanged);
    assert_eq!(got.body, Some(screen("new")));
    assert_eq!(got.rev, rev_of(&screen("new")));
    assert_eq!(
        a.text_reads, 2,
        "the first read and the one after the change"
    );
    assert_eq!(*clock.sleeps.borrow(), vec![250, 250]);
}

#[test]
fn a_changed_marker_with_the_same_screen_keeps_waiting() {
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    a.text = "same".into();
    a.markers = vec!["m1".into(), "m2".into(), "m3".into()];
    let clock = FakeClock::default();
    let got = read_screen(&mut a, &clock, &read_poll("same", 500), "w1:p1", 2000).unwrap();
    assert!(got.unchanged);
    assert_eq!(a.text_reads, 3);
}

#[test]
fn without_a_marker_the_pane_is_read_every_tick() {
    // An older Herdr without `pane.get`: today's behavior.
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    a.text = "hello".into();
    let clock = FakeClock::default();
    let got = read_screen(&mut a, &clock, &read_poll("hello", 500), "w1:p1", 2000).unwrap();
    assert!(got.unchanged);
    assert_eq!(a.text_reads, 3);
}

#[test]
fn a_pane_closed_while_waiting_is_reported() {
    let mut a = api(vec![snap(AgentStatus::Idle), NavSnapshot::default()]);
    a.text = "hello".into();
    a.markers = vec!["m1".into(), "gone".into()];
    let err = read_screen(
        &mut a,
        &FakeClock::default(),
        &read_poll("hello", 1000),
        "w1:p1",
        2000,
    )
    .unwrap_err();
    assert_eq!(err, NavError::Invalid("pane not found".into()));
}

#[test]
fn read_refuses_a_pane_missing_from_the_snapshot() {
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    let err =
        read_screen(&mut a, &FakeClock::default(), &Poll::default(), "w1:p9", 10).unwrap_err();
    assert_eq!(err, NavError::Invalid("pane not found".into()));
    assert_eq!(a.calls, vec!["snapshot"], "never read");
}

fn run(
    op: &str,
    session: Option<&str>,
) -> (Result<ActionDone, NavError>, FakeApi, Vec<Vec<String>>) {
    let _ = session;
    let mut a = api(vec![snap(AgentStatus::Idle)]);
    let r = RecordingRunner::default();
    let got = Op::parse(op).and_then(|op| execute(&mut a, &op));
    (got, a, r.argv())
}

#[test]
fn socket_actions_are_validated_then_applied() {
    let (got, a, argv) = run(r#"{"op":"workspace.create","label":"api"}"#, None);
    assert_eq!(
        got.unwrap(),
        ActionDone {
            op: "workspace.create",
            id: Some("w2".into())
        }
    );
    assert_eq!(a.calls.len(), 2);
    assert!(
        a.calls[1].starts_with("apply WorkspaceCreate"),
        "{:?}",
        a.calls
    );
    assert!(argv.is_empty());

    let (got, a, _) = run(r#"{"op":"tab.close","id":"w1:t1"}"#, None);
    assert_eq!(got.unwrap().id, None);
    assert!(a.calls[1].starts_with("apply TabClose"));
}

#[test]
fn rejected_actions_never_reach_herdr() {
    let (got, a, argv) = run(r#"{"op":"tab.close","id":"w1:t7"}"#, None);
    assert_eq!(
        got.unwrap_err().to_string(),
        "invalid request: tab not found"
    );
    assert_eq!(a.calls, vec!["snapshot"]);
    assert!(argv.is_empty());
}

#[test]
fn text_goes_over_the_socket_then_enter() {
    // Over SSH the `herdr` binary is often not on PATH; the socket always is.
    let (got, a, argv) = run(
        r#"{"op":"pane.text","pane_id":"w1:p1","text":"-rf \u001b[2Jnow\nok","submit":true}"#,
        Some("work"),
    );
    assert_eq!(
        got.unwrap(),
        ActionDone {
            op: "pane.text",
            id: None
        }
    );
    assert!(argv.is_empty(), "no CLI call: {argv:?}");
    assert!(
        a.calls[1].contains("PaneText") && a.calls[1].contains("-rf [2Jnow ok"),
        "{:?}",
        a.calls
    );
    assert!(
        a.calls[2].contains("PaneKeys") && a.calls[2].contains("\"enter\""),
        "{:?}",
        a.calls
    );

    let (_, a, argv) = run(r#"{"op":"pane.text","pane_id":"w1:p1","text":"hi"}"#, None);
    assert!(argv.is_empty());
    assert_eq!(a.calls.len(), 2, "no Enter without submit: {:?}", a.calls);

    let (got, a, _) = run(
        r#"{"op":"pane.text","pane_id":"w1:p1","text":"\u0007"}"#,
        None,
    );
    assert_eq!(got.unwrap_err().code(), "invalid_request");
    assert_eq!(a.calls, vec!["snapshot"]);
}
