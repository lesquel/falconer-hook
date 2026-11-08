//! Shared adapters: crypto primitives, secure filesystem helpers, process
//! runner, redacting logger, host identity.

pub mod crypto;
pub mod fs_secure;
pub mod host_identity;
pub mod log;
pub mod process;
pub mod ssh_fingerprint;
pub mod system;
