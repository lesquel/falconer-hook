//! `inbox list`: page through events by change sequence.

use serde::Serialize;

use crate::features::events::domain::{Event, EventStore, StoreError};

pub const DEFAULT_LIMIT: u32 = 100;
pub const MAX_LIMIT: u32 = 500;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct InboxPage {
    pub events: Vec<Event>,
    /// Highest `seq` in this page, or the requested `since` when empty.
    pub last_seq: u64,
}

pub struct ListInbox<'a, S> {
    pub store: &'a S,
}

impl<S: EventStore> ListInbox<'_, S> {
    pub fn list(&self, since: Option<u64>, limit: Option<u32>) -> Result<InboxPage, StoreError> {
        let since = since.unwrap_or(0);
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let events = self.store.list_since(since, limit)?;
        let last_seq = events.last().map_or(since, |e| e.seq);
        Ok(InboxPage { events, last_seq })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::events::domain::fakes::MemStore;
    use crate::features::events::domain::{EventKind, NewEvent};

    #[test]
    fn pages_by_seq_and_clamps_limit() {
        let store = MemStore::default();
        for i in 0..3 {
            store
                .insert(
                    &format!("e{i}"),
                    &NewEvent {
                        source: "s".into(),
                        agent: "a".into(),
                        kind: EventKind::Done,
                        title: String::new(),
                        summary: String::new(),
                        pane: None,
                        created_at: 1,
                    },
                )
                .unwrap();
        }
        let uc = ListInbox { store: &store };
        let p = uc.list(Some(1), Some(1)).unwrap();
        assert_eq!(p.events.len(), 1);
        assert_eq!(p.last_seq, 2);
        assert_eq!(
            uc.list(None, Some(0)).unwrap().events.len(),
            1,
            "limit 0 is clamped to 1"
        );
        assert_eq!(
            uc.list(Some(3), None).unwrap(),
            InboxPage {
                events: vec![],
                last_seq: 3
            }
        );
    }
}
