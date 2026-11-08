//! Record a new event (with dedupe and retention) and clear stale ones.

use crate::features::events::domain::{
    is_duplicate, retention_cutoff, Event, EventKind, EventStore, NewEvent, StoreError,
};
use crate::shared::domain::clock::Clock;
use crate::shared::domain::ids::IdGenerator;

#[derive(Debug, PartialEq, Eq)]
pub enum IngestOutcome {
    /// Stored. `superseded` are older pending events on the same pane that
    /// were resolved because the pane moved on.
    Recorded {
        event: Box<Event>,
        superseded: Vec<Event>,
    },
    /// Dropped: repeats a pending event on the same pane within 10 s.
    Duplicate,
}

pub struct IngestEvent<'a, S, I, C> {
    pub store: &'a S,
    pub ids: &'a I,
    pub clock: &'a C,
    pub retention_days: u32,
}

impl<S: EventStore, I: IdGenerator, C: Clock> IngestEvent<'_, S, I, C> {
    pub fn ingest(&self, new: NewEvent) -> Result<IngestOutcome, StoreError> {
        let now = self.clock.now();
        self.store
            .purge_before(retention_cutoff(now, self.retention_days))?;
        let mut superseded = Vec::new();
        if let Some(pane) = &new.pane {
            let last = self.store.last_for(&pane.pane, new.kind)?;
            if is_duplicate(last.as_ref(), &new) {
                return Ok(IngestOutcome::Duplicate);
            }
            for old in self.store.pending_for_pane(&pane.pane)? {
                superseded.push(self.store.resolve(&old.id, "superseded", now)?);
            }
        }
        let event = Box::new(self.store.insert(&self.ids.new_id(), &new)?);
        Ok(IngestOutcome::Recorded { event, superseded })
    }
}

/// Resolves pending approvals/questions on a pane that was answered on the
/// host directly (e.g. Herdr reports it left `blocked`).
pub struct ClearPane<'a, S, C> {
    pub store: &'a S,
    pub clock: &'a C,
}

impl<S: EventStore, C: Clock> ClearPane<'_, S, C> {
    pub fn clear(&self, pane: &str) -> Result<Vec<Event>, StoreError> {
        let now = self.clock.now();
        let mut out = Vec::new();
        for e in self.store.pending_for_pane(pane)? {
            if matches!(e.kind, EventKind::ApprovalRequired | EventKind::Question) {
                out.push(self.store.resolve(&e.id, "handled_on_host", now)?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::events::domain::fakes::MemStore;
    use crate::features::events::domain::{EventStatus, PaneRef, Runtime, SECS_PER_DAY};
    use crate::shared::domain::clock::fakes::FixedClock;
    use crate::shared::domain::ids::fakes::SeqIds;

    fn new(kind: EventKind, pane: Option<&str>, at: u64) -> NewEvent {
        NewEvent {
            source: "herdr".into(),
            agent: "claude".into(),
            kind,
            title: "t".into(),
            summary: "s".into(),
            pane: pane.map(|p| PaneRef {
                runtime: Runtime::Herdr,
                pane: p.into(),
            }),
            created_at: at,
        }
    }

    #[test]
    fn records_dedupes_and_supersedes() {
        let store = MemStore::default();
        let clock = FixedClock::at(1000);
        let ids = SeqIds::default();
        let uc = IngestEvent {
            store: &store,
            ids: &ids,
            clock: &clock,
            retention_days: 7,
        };

        let IngestOutcome::Recorded { event: first, .. } = uc
            .ingest(new(EventKind::ApprovalRequired, Some("p"), 1000))
            .unwrap()
        else {
            panic!("first event must be recorded")
        };
        assert_eq!(first.seq, 1);
        assert_eq!(
            uc.ingest(new(EventKind::ApprovalRequired, Some("p"), 1005))
                .unwrap(),
            IngestOutcome::Duplicate
        );

        clock.advance(20);
        let IngestOutcome::Recorded { superseded, event } =
            uc.ingest(new(EventKind::Done, Some("p"), 1020)).unwrap()
        else {
            panic!("done must be recorded")
        };
        assert_eq!(superseded.len(), 1);
        assert_eq!(superseded[0].id, first.id);
        assert_eq!(superseded[0].resolution.as_deref(), Some("superseded"));
        assert_eq!(event.status, EventStatus::Pending);
        // Events without a pane are never deduplicated.
        assert!(matches!(
            uc.ingest(new(EventKind::Done, None, 1020)).unwrap(),
            IngestOutcome::Recorded { .. }
        ));
        assert!(matches!(
            uc.ingest(new(EventKind::Done, None, 1020)).unwrap(),
            IngestOutcome::Recorded { .. }
        ));
    }

    #[test]
    fn expired_events_are_purged_on_ingest() {
        let store = MemStore::default();
        let clock = FixedClock::at(100);
        let ids = SeqIds::default();
        let uc = IngestEvent {
            store: &store,
            ids: &ids,
            clock: &clock,
            retention_days: 7,
        };
        uc.ingest(new(EventKind::Done, Some("a"), 100)).unwrap();
        clock.advance(8 * SECS_PER_DAY);
        uc.ingest(new(EventKind::Done, Some("b"), clock.now()))
            .unwrap();
        let all = store.list_since(0, 100).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pane.as_deref(), Some("b"));
    }

    #[test]
    fn clear_pane_resolves_only_prompts() {
        let store = MemStore::default();
        let clock = FixedClock::at(10);
        let ids = SeqIds::default();
        store
            .insert(
                &ids.new_id(),
                &new(EventKind::ApprovalRequired, Some("p"), 10),
            )
            .unwrap();
        store
            .insert(&ids.new_id(), &new(EventKind::Done, Some("p"), 10))
            .unwrap();
        let cleared = ClearPane {
            store: &store,
            clock: &clock,
        }
        .clear("p")
        .unwrap();
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].resolution.as_deref(), Some("handled_on_host"));
    }
}
