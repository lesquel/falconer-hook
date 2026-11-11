//! `respond <event-id> --action allow|deny|text [--text <msg>]`.

use serde::Serialize;

use crate::features::agents::domain::{AgentError, AgentRuntime};
use crate::features::events::domain::{
    EventKind, EventNotifier, EventStatus, EventStore, StoreError,
};
use crate::features::responses::domain::{
    check_age, check_target, sanitize_text, Action, KeymapSource, ResponseError,
};
use crate::shared::domain::clock::Clock;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RespondOutcome {
    pub event_id: String,
    pub action: Action,
    pub seq: u64,
    /// Devices that received the `resolved` push.
    pub pushed: u32,
}

pub struct RespondToEvent<'a, S, K, C> {
    pub store: &'a S,
    pub keymaps: &'a K,
    pub clock: &'a C,
    pub runtimes: &'a [&'a dyn AgentRuntime],
    pub notifier: &'a dyn EventNotifier,
}

fn store_err(e: StoreError) -> ResponseError {
    match e {
        StoreError::NotFound => ResponseError::NotFound,
        StoreError::Storage => ResponseError::Store,
    }
}

impl<S: EventStore, K: KeymapSource, C: Clock> RespondToEvent<'_, S, K, C> {
    pub fn respond(
        &self,
        event_id: &str,
        action: Action,
        text: Option<&str>,
    ) -> Result<RespondOutcome, ResponseError> {
        let text = match (action, text) {
            (Action::Text, Some(t)) => Some(sanitize_text(t)?),
            (Action::Text, None) | (_, Some(_)) => return Err(ResponseError::TextArgument),
            _ => None,
        };
        let event = self
            .store
            .get(event_id)
            .map_err(store_err)?
            .ok_or(ResponseError::NotFound)?;
        if event.status != EventStatus::Pending {
            return Err(ResponseError::AlreadyResolved);
        }
        if matches!(action, Action::Allow | Action::Deny)
            && event.kind != EventKind::ApprovalRequired
        {
            return Err(ResponseError::InvalidAction);
        }
        check_age(&event, self.clock.now())?;
        let pane = event.pane_ref().ok_or(ResponseError::NoPane)?;
        let runtime = self
            .runtimes
            .iter()
            .find(|r| r.runtime() == pane.runtime)
            .ok_or(ResponseError::RuntimeUnavailable)?;
        let keymap = self
            .keymaps
            .keymap(&event.agent)
            .ok_or(ResponseError::NoKeymap)?;
        // Re-check the pane right before anything is sent: it must still be
        // the same pane, running the same agent, showing the same prompt.
        let probe = runtime.probe(&pane).map_err(|e| match e {
            AgentError::Unavailable(_) => ResponseError::RuntimeUnavailable,
            _ => ResponseError::Delivery,
        })?;
        check_target(&event, &probe)?;
        // Claim first: an atomic `pending -> resolved` compare-and-set. Only
        // the winner of concurrent responses sends anything.
        let resolved = match self
            .store
            .resolve(event_id, action.as_str(), self.clock.now())
        {
            Ok(e) => e,
            Err(StoreError::NotFound) => return Err(ResponseError::AlreadyResolved),
            Err(e) => return Err(store_err(e)),
        };
        let delivered = match action {
            Action::Allow => runtime.send_keys(&pane, &keymap.allow),
            Action::Deny => runtime.send_keys(&pane, &keymap.deny),
            Action::Text => {
                let t = text.as_deref().unwrap_or_default();
                runtime.send_text(&pane, t).and_then(|_| {
                    if keymap.submit.is_empty() {
                        Ok(())
                    } else {
                        runtime.send_keys(&pane, &keymap.submit)
                    }
                })
            }
        };
        if let Err(e) = delivered {
            // Nothing reached the agent: give the event back so it can be retried.
            let _ = self.store.reopen(event_id);
            return Err(match e {
                AgentError::Unavailable(_) => ResponseError::RuntimeUnavailable,
                _ => ResponseError::Delivery,
            });
        }
        let pushed = self.notifier.notify_resolved(&resolved);
        Ok(RespondOutcome {
            event_id: resolved.id,
            action,
            seq: resolved.seq,
            pushed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agents::domain::fakes::FakeRuntime;
    use crate::features::agents::domain::{AgentStatus, PaneState, Runtime};
    use crate::features::events::domain::fakes::{MemStore, RecordingNotifier};
    use crate::features::events::domain::{Event, NewEvent, PaneRef};
    use crate::features::responses::domain::Keymap;
    use crate::shared::domain::clock::fakes::FixedClock;
    use crate::shared::domain::error::ErrorCode;

    struct Maps;
    impl KeymapSource for Maps {
        fn keymap(&self, agent: &str) -> Option<Keymap> {
            (agent == "claude").then(|| Keymap {
                agent: "claude".into(),
                version: 1,
                verified: false,
                allow: vec!["enter".into()],
                deny: vec!["esc".into()],
                submit: vec!["enter".into()],
            })
        }
    }

    fn seed(
        store: &MemStore,
        id: &str,
        kind: EventKind,
        agent: &str,
        pane: Option<(Runtime, &str)>,
    ) {
        let e = NewEvent {
            source: "herdr".into(),
            agent: agent.into(),
            kind,
            title: "t".into(),
            summary: "s".into(),
            pane: pane.map(|(runtime, p)| PaneRef::new(runtime, p)),
            created_at: 1,
        };
        store.insert(id, &e).unwrap();
    }

    struct Rig {
        store: MemStore,
        herdr: FakeRuntime,
        clock: FixedClock,
        notifier: RecordingNotifier,
    }

    impl Rig {
        fn new() -> Self {
            let store = MemStore::default();
            seed(
                &store,
                "a",
                EventKind::ApprovalRequired,
                "claude",
                Some((Runtime::Herdr, "w1:p1")),
            );
            seed(
                &store,
                "q",
                EventKind::Question,
                "claude",
                Some((Runtime::Herdr, "w1:p2")),
            );
            seed(&store, "n", EventKind::ApprovalRequired, "claude", None);
            seed(
                &store,
                "x",
                EventKind::ApprovalRequired,
                "mystery",
                Some((Runtime::Herdr, "w1:p3")),
            );
            seed(
                &store,
                "t",
                EventKind::ApprovalRequired,
                "claude",
                Some((Runtime::Tmux, "%1")),
            );
            let herdr = FakeRuntime::herdr_with_agents(0);
            for p in ["w1:p1", "w1:p2", "w1:p3"] {
                herdr.live(p, blocked("claude"));
            }
            Self {
                store,
                herdr,
                clock: FixedClock::at(50),
                notifier: RecordingNotifier::default(),
            }
        }
        fn run(
            &self,
            id: &str,
            action: Action,
            text: Option<&str>,
        ) -> Result<RespondOutcome, ResponseError> {
            let rts: [&dyn AgentRuntime; 1] = [&self.herdr];
            let uc = RespondToEvent {
                store: &self.store,
                keymaps: &Maps,
                clock: &self.clock,
                runtimes: &rts,
                notifier: &self.notifier,
            };
            uc.respond(id, action, text)
        }
    }

    fn blocked(agent: &str) -> PaneState {
        PaneState {
            pane_pid: None,
            agent: Some(agent.into()),
            status: Some(AgentStatus::Blocked),
            state_change_seq: Some(1),
        }
    }

    fn sent(pane: &str, key: &str) -> (String, Vec<String>) {
        (pane.to_string(), vec![key.to_string()])
    }

    #[test]
    fn allow_sends_keymap_keys_resolves_and_notifies() {
        let r = Rig::new();
        let out = r.run("a", Action::Allow, None).unwrap();
        assert_eq!(r.herdr.sent.borrow().as_slice(), [sent("w1:p1", "enter")]);
        assert_eq!((out.event_id.as_str(), out.pushed), ("a", 1));
        let e = r.store.get("a").unwrap().unwrap();
        assert_eq!(e.status, EventStatus::Resolved);
        assert_eq!(e.resolution.as_deref(), Some("allow"));
        assert_eq!(e.resolved_at, Some(50));
        assert_eq!(r.notifier.resolved.borrow().as_slice(), ["a"]);
        assert_eq!(
            r.run("a", Action::Allow, None).unwrap_err(),
            ResponseError::AlreadyResolved
        );
    }

    #[test]
    fn deny_and_text() {
        let r = Rig::new();
        r.run("a", Action::Deny, None).unwrap();
        r.run("q", Action::Text, Some("run it\u{1b}[2J again\nplease"))
            .unwrap();
        assert_eq!(
            r.herdr.sent.borrow().as_slice(),
            [sent("w1:p1", "esc"), sent("w1:p2", "enter")]
        );
        assert_eq!(
            r.herdr.typed.borrow().as_slice(),
            [("w1:p2".to_string(), "run it[2J again please".to_string())]
        );
    }

    #[test]
    fn rejections() {
        let r = Rig::new();
        let code = |res: Result<RespondOutcome, ResponseError>| res.unwrap_err().code();
        assert_eq!(code(r.run("missing", Action::Allow, None)), "not_found");
        assert_eq!(code(r.run("q", Action::Allow, None)), "invalid_input");
        assert_eq!(code(r.run("a", Action::Text, None)), "invalid_input");
        assert_eq!(code(r.run("a", Action::Allow, Some("x"))), "invalid_input");
        assert_eq!(
            code(r.run("a", Action::Text, Some(&"a".repeat(16385)))),
            "text_too_long"
        );
        assert_eq!(code(r.run("n", Action::Allow, None)), "no_pane");
        assert_eq!(code(r.run("x", Action::Allow, None)), "no_keymap");
        assert_eq!(code(r.run("t", Action::Allow, None)), "runtime_unavailable");
        assert!(
            r.herdr.sent.borrow().is_empty(),
            "nothing is sent on rejection"
        );
        assert!(r.notifier.resolved.borrow().is_empty());
    }

    #[test]
    fn stale_gone_and_expired_panes_are_refused_and_stay_pending() {
        let r = Rig::new();
        let code = |res: Result<RespondOutcome, ResponseError>| res.unwrap_err().code();
        // The agent moved on (now working): stale.
        r.herdr.live(
            "w1:p1",
            PaneState {
                status: Some(AgentStatus::Working),
                ..blocked("claude")
            },
        );
        assert_eq!(code(r.run("a", Action::Allow, None)), "stale");
        // Another agent took the pane over: stale.
        r.herdr.live("w1:p1", blocked("codex"));
        assert_eq!(code(r.run("a", Action::Allow, None)), "stale");
        // The pane is gone: no_pane.
        r.herdr.probes.borrow_mut().remove("w1:p1");
        assert_eq!(code(r.run("a", Action::Allow, None)), "no_pane");
        r.herdr.live("w1:p1", blocked("claude"));
        // Fifteen minutes later the approval can no longer be answered.
        r.clock.advance(15 * 60 + 1);
        assert_eq!(code(r.run("a", Action::Allow, None)), "expired");
        assert!(r.herdr.sent.borrow().is_empty());
        assert_eq!(
            r.store.get("a").unwrap().unwrap().status,
            EventStatus::Pending
        );
    }

    #[test]
    fn tmux_answers_go_to_the_recorded_socket_and_need_the_same_pid() {
        let store = MemStore::default();
        let mut e = NewEvent {
            source: "claude".into(),
            agent: "claude".into(),
            kind: EventKind::ApprovalRequired,
            title: "t".into(),
            summary: "s".into(),
            pane: Some(PaneRef::new(Runtime::Tmux, "%4")),
            created_at: 40,
        };
        if let Some(p) = e.pane.as_mut() {
            p.context.session = Some("/tmp/tmux-1000/work".into());
            p.context.pane_pid = Some(77);
        }
        store.insert("t", &e).unwrap();
        let tmux = FakeRuntime::tmux();
        let (clock, notifier) = (FixedClock::at(50), RecordingNotifier::default());
        let rts: [&dyn AgentRuntime; 1] = [&tmux];
        let uc = RespondToEvent {
            store: &store,
            keymaps: &Maps,
            clock: &clock,
            runtimes: &rts,
            notifier: &notifier,
        };
        tmux.live(
            "%4",
            PaneState {
                pane_pid: Some(78),
                ..Default::default()
            },
        );
        assert_eq!(
            uc.respond("t", Action::Allow, None),
            Err(ResponseError::Stale)
        );
        tmux.live(
            "%4",
            PaneState {
                pane_pid: Some(77),
                ..Default::default()
            },
        );
        uc.respond("t", Action::Allow, None).unwrap();
        assert_eq!(tmux.sent.borrow().as_slice(), [sent("%4", "enter")]);
        assert!(tmux
            .sessions_seen
            .borrow()
            .iter()
            .all(|s| s.as_deref() == Some("/tmp/tmux-1000/work")));
    }

    /// Simulates another `respond` resolving the event between our read and our claim.
    struct RacyStore(MemStore);
    impl EventStore for RacyStore {
        fn insert(&self, id: &str, e: &NewEvent) -> Result<Event, StoreError> {
            self.0.insert(id, e)
        }
        fn get(&self, id: &str) -> Result<Option<Event>, StoreError> {
            let stale = self.0.get(id)?;
            if stale
                .as_ref()
                .is_some_and(|e| e.status == EventStatus::Pending)
            {
                self.0.resolve(id, "deny", 1)?;
            }
            Ok(stale)
        }
        fn list_since(&self, s: u64, l: u32) -> Result<Vec<Event>, StoreError> {
            self.0.list_since(s, l)
        }
        fn last_for(&self, p: &str, k: EventKind) -> Result<Option<Event>, StoreError> {
            self.0.last_for(p, k)
        }
        fn pending_for_pane(&self, p: &str) -> Result<Vec<Event>, StoreError> {
            self.0.pending_for_pane(p)
        }
        fn resolve(&self, id: &str, r: &str, at: u64) -> Result<Event, StoreError> {
            self.0.resolve(id, r, at)
        }
        fn reopen(&self, id: &str) -> Result<(), StoreError> {
            self.0.reopen(id)
        }
        fn purge_before(&self, ts: u64) -> Result<u64, StoreError> {
            self.0.purge_before(ts)
        }
    }

    #[test]
    fn concurrent_loser_sends_nothing() {
        let r = Rig::new();
        r.herdr.live("w1:p1", blocked("claude"));
        let racy = RacyStore(MemStore::default());
        seed(
            &racy.0,
            "a",
            EventKind::ApprovalRequired,
            "claude",
            Some((Runtime::Herdr, "w1:p1")),
        );
        let rts: [&dyn AgentRuntime; 1] = [&r.herdr];
        let uc = RespondToEvent {
            store: &racy,
            keymaps: &Maps,
            clock: &r.clock,
            runtimes: &rts,
            notifier: &r.notifier,
        };
        let err = uc.respond("a", Action::Allow, None).unwrap_err();
        assert_eq!(err, ResponseError::AlreadyResolved);
        assert_eq!(err.code(), "already_resolved");
        assert!(r.herdr.sent.borrow().is_empty(), "loser must not send keys");
        assert!(r.notifier.resolved.borrow().is_empty());
        assert_eq!(
            racy.0.get("a").unwrap().unwrap().resolution.as_deref(),
            Some("deny")
        );
    }

    #[test]
    fn delivery_failure_leaves_event_pending() {
        let mut r = Rig::new();
        r.herdr = FakeRuntime::unavailable(Runtime::Herdr);
        assert_eq!(
            r.run("a", Action::Allow, None).unwrap_err(),
            ResponseError::RuntimeUnavailable
        );
        assert_eq!(
            r.store.get("a").unwrap().unwrap().status,
            EventStatus::Pending
        );
    }
}
