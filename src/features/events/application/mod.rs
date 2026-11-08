//! Event use cases.

mod inbox;
mod ingest;

pub use inbox::ListInbox;
pub use ingest::{ClearPane, IngestEvent, IngestOutcome};
