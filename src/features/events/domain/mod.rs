//! Event entities, rules and the `EventStore` port.

mod hook_payload;

pub use hook_payload::{parse_hook_payload, parse_with_kind, EmitKind};

use serde::Serialize;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

/// Events closer than this (same pane and kind, previous still pending) are dropped.
pub const DEDUPE_WINDOW_SECS: u64 = 10;
pub const SECS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ApprovalRequired,
    Question,
    Done,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ApprovalRequired => "approval_required",
            EventKind::Question => "question",
            EventKind::Done => "done",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approval_required" => Some(EventKind::ApprovalRequired),
            "question" => Some(EventKind::Question),
            "done" => Some(EventKind::Done),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Pending,
    Resolved,
}

/// Where the agent's pane lives, so a response can be delivered.
pub use crate::shared::domain::runtime::Runtime;

/// An agent pane reference (`w7:pA` for Herdr, `%3` for tmux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneRef {
    pub runtime: Runtime,
    pub pane: String,
}

/// Pane ids end up as argv entries; restrict them to a safe alphabet anyway.
pub fn valid_pane_id(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && !p.starts_with('-')
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":%._-@$".contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    pub source: String,
    pub agent: String,
    pub kind: EventKind,
    pub title: String,
    pub summary: String,
    pub pane: Option<PaneRef>,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub id: String,
    /// Monotonic change sequence: assigned on insert and bumped on resolution,
    /// so `inbox list --since` also returns resolutions.
    pub seq: u64,
    pub source: String,
    pub agent: String,
    pub kind: EventKind,
    pub status: EventStatus,
    pub title: String,
    pub summary: String,
    pub pane: Option<String>,
    pub runtime: Option<Runtime>,
    pub created_at: u64,
    pub resolved_at: Option<u64>,
    pub resolution: Option<String>,
}

impl Event {
    pub fn pane_ref(&self) -> Option<PaneRef> {
        Some(PaneRef {
            runtime: self.runtime?,
            pane: self.pane.clone()?,
        })
    }
}

/// True when `new` repeats `last` (same pane and kind, still pending, within the window).
pub fn is_duplicate(last: Option<&Event>, new: &NewEvent) -> bool {
    let (Some(last), Some(pane)) = (last, new.pane.as_ref()) else {
        return false;
    };
    last.status == EventStatus::Pending
        && last.kind == new.kind
        && last.pane.as_deref() == Some(pane.pane.as_str())
        && new.created_at.saturating_sub(last.created_at) <= DEDUPE_WINDOW_SECS
}

/// Events created before the returned timestamp are expired.
pub fn retention_cutoff(now: u64, retention_days: u32) -> u64 {
    now.saturating_sub(u64::from(retention_days) * SECS_PER_DAY)
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("event not found")]
    NotFound,
    #[error("event store failure")]
    Storage,
}

impl ErrorCode for StoreError {
    fn code(&self) -> &'static str {
        match self {
            StoreError::NotFound => "not_found",
            StoreError::Storage => "store_error",
        }
    }
}

/// Persistent inbox.
pub trait EventStore {
    fn insert(&self, id: &str, event: &NewEvent) -> Result<Event, StoreError>;
    fn get(&self, id: &str) -> Result<Option<Event>, StoreError>;
    /// Events with `seq > since`, ascending, at most `limit`.
    fn list_since(&self, since: u64, limit: u32) -> Result<Vec<Event>, StoreError>;
    /// Most recent event for `pane` and `kind`.
    fn last_for(&self, pane: &str, kind: EventKind) -> Result<Option<Event>, StoreError>;
    /// Pending events on `pane`.
    fn pending_for_pane(&self, pane: &str) -> Result<Vec<Event>, StoreError>;
    /// Atomically marks a *pending* event resolved and assigns it a new `seq`
    /// (compare-and-set). `NotFound` when it is missing or no longer pending.
    fn resolve(&self, id: &str, resolution: &str, at: u64) -> Result<Event, StoreError>;
    /// Returns a resolved event to pending with a new `seq` (undo of a claim
    /// whose delivery failed).
    fn reopen(&self, id: &str) -> Result<(), StoreError>;
    /// Deletes events created before `ts`. Returns how many were removed.
    fn purge_before(&self, ts: u64) -> Result<u64, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind, pane: &str, at: u64, status: EventStatus) -> Event {
        Event {
            id: "x".into(),
            seq: 1,
            source: "herdr".into(),
            agent: "claude".into(),
            kind,
            status,
            title: String::new(),
            summary: String::new(),
            pane: Some(pane.into()),
            runtime: Some(Runtime::Herdr),
            created_at: at,
            resolved_at: None,
            resolution: None,
        }
    }

    fn new(kind: EventKind, pane: Option<&str>, at: u64) -> NewEvent {
        NewEvent {
            source: "herdr".into(),
            agent: "claude".into(),
            kind,
            title: String::new(),
            summary: String::new(),
            pane: pane.map(|p| PaneRef {
                runtime: Runtime::Herdr,
                pane: p.into(),
            }),
            created_at: at,
        }
    }

    #[test]
    fn same_pane_and_kind_within_ten_seconds_is_duplicate() {
        let last = ev(
            EventKind::ApprovalRequired,
            "w1:p1",
            100,
            EventStatus::Pending,
        );
        assert!(is_duplicate(
            Some(&last),
            &new(EventKind::ApprovalRequired, Some("w1:p1"), 110)
        ));
        assert!(!is_duplicate(
            Some(&last),
            &new(EventKind::ApprovalRequired, Some("w1:p1"), 111)
        ));
        assert!(!is_duplicate(
            Some(&last),
            &new(EventKind::Done, Some("w1:p1"), 105)
        ));
        assert!(!is_duplicate(
            Some(&last),
            &new(EventKind::ApprovalRequired, Some("w1:p2"), 105)
        ));
        assert!(!is_duplicate(
            Some(&last),
            &new(EventKind::ApprovalRequired, None, 105)
        ));
        assert!(!is_duplicate(
            None,
            &new(EventKind::ApprovalRequired, Some("w1:p1"), 105)
        ));
    }

    #[test]
    fn resolved_event_never_suppresses_a_new_one() {
        let last = ev(
            EventKind::ApprovalRequired,
            "w1:p1",
            100,
            EventStatus::Resolved,
        );
        assert!(!is_duplicate(
            Some(&last),
            &new(EventKind::ApprovalRequired, Some("w1:p1"), 101)
        ));
    }

    #[test]
    fn retention_is_days_back() {
        assert_eq!(retention_cutoff(10 * SECS_PER_DAY, 7), 3 * SECS_PER_DAY);
        assert_eq!(retention_cutoff(5, 7), 0);
    }

    #[test]
    fn pane_ids_are_restricted() {
        for ok in ["w7:pA", "%3", "main:1.0", "wS:p1"] {
            assert!(valid_pane_id(ok), "{ok}");
        }
        for bad in ["", "-t", "a b", "a;b", "a\nb", &"x".repeat(65)] {
            assert!(!valid_pane_id(bad), "{bad:?}");
        }
    }
}

/// Tells paired devices about an event (implemented by the push feature).
/// Returns how many devices received it. Delivery failures never fail the caller.
pub trait EventNotifier {
    fn notify(&self, event: &Event) -> u32;
    /// Publishes a `resolved` push so other devices clear the notification.
    fn notify_resolved(&self, event: &Event) -> u32;
}

#[cfg(test)]
pub mod fakes {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct RecordingNotifier {
        pub notified: RefCell<Vec<String>>,
        pub resolved: RefCell<Vec<String>>,
    }

    impl EventNotifier for RecordingNotifier {
        fn notify(&self, event: &Event) -> u32 {
            self.notified.borrow_mut().push(event.id.clone());
            1
        }
        fn notify_resolved(&self, event: &Event) -> u32 {
            self.resolved.borrow_mut().push(event.id.clone());
            1
        }
    }

    /// In-memory `EventStore` with the same seq semantics as SQLite.
    #[derive(Default)]
    pub struct MemStore {
        pub events: RefCell<Vec<Event>>,
        pub next: RefCell<u64>,
    }

    impl MemStore {
        fn bump(&self) -> u64 {
            let mut n = self.next.borrow_mut();
            *n += 1;
            *n
        }
    }

    impl EventStore for MemStore {
        fn insert(&self, id: &str, e: &NewEvent) -> Result<Event, StoreError> {
            let ev = Event {
                id: id.into(),
                seq: self.bump(),
                source: e.source.clone(),
                agent: e.agent.clone(),
                kind: e.kind,
                status: EventStatus::Pending,
                title: e.title.clone(),
                summary: e.summary.clone(),
                pane: e.pane.as_ref().map(|p| p.pane.clone()),
                runtime: e.pane.as_ref().map(|p| p.runtime),
                created_at: e.created_at,
                resolved_at: None,
                resolution: None,
            };
            self.events.borrow_mut().push(ev.clone());
            Ok(ev)
        }
        fn get(&self, id: &str) -> Result<Option<Event>, StoreError> {
            Ok(self.events.borrow().iter().find(|e| e.id == id).cloned())
        }
        fn list_since(&self, since: u64, limit: u32) -> Result<Vec<Event>, StoreError> {
            let mut v: Vec<_> = self
                .events
                .borrow()
                .iter()
                .filter(|e| e.seq > since)
                .cloned()
                .collect();
            v.sort_by_key(|e| e.seq);
            v.truncate(limit as usize);
            Ok(v)
        }
        fn last_for(&self, pane: &str, kind: EventKind) -> Result<Option<Event>, StoreError> {
            Ok(self
                .events
                .borrow()
                .iter()
                .filter(|e| e.pane.as_deref() == Some(pane) && e.kind == kind)
                .max_by_key(|e| (e.created_at, e.seq))
                .cloned())
        }
        fn pending_for_pane(&self, pane: &str) -> Result<Vec<Event>, StoreError> {
            Ok(self
                .events
                .borrow()
                .iter()
                .filter(|e| e.pane.as_deref() == Some(pane) && e.status == EventStatus::Pending)
                .cloned()
                .collect())
        }
        fn resolve(&self, id: &str, resolution: &str, at: u64) -> Result<Event, StoreError> {
            let seq = self.bump();
            let mut v = self.events.borrow_mut();
            let e = v
                .iter_mut()
                .find(|e| e.id == id && e.status == EventStatus::Pending)
                .ok_or(StoreError::NotFound)?;
            e.status = EventStatus::Resolved;
            e.seq = seq;
            e.resolved_at = Some(at);
            e.resolution = Some(resolution.into());
            Ok(e.clone())
        }
        fn reopen(&self, id: &str) -> Result<(), StoreError> {
            let seq = self.bump();
            let mut v = self.events.borrow_mut();
            let e = v
                .iter_mut()
                .find(|e| e.id == id && e.status == EventStatus::Resolved)
                .ok_or(StoreError::NotFound)?;
            e.status = EventStatus::Pending;
            e.seq = seq;
            e.resolved_at = None;
            e.resolution = None;
            Ok(())
        }
        fn purge_before(&self, ts: u64) -> Result<u64, StoreError> {
            let mut v = self.events.borrow_mut();
            let before = v.len();
            v.retain(|e| e.created_at >= ts);
            Ok((before - v.len()) as u64)
        }
    }
}
