//! Event use cases.

mod inbox;
mod ingest;

pub use inbox::{InboxPage, ListInbox, DEFAULT_LIMIT, MAX_LIMIT};
pub use ingest::{ClearPane, IngestEvent, IngestOutcome};
