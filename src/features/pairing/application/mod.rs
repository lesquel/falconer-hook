//! Pairing use cases.

mod devices;
mod qr;
mod ssh;

pub use devices::{DeviceAdmin, DeviceAdminError, DeviceView};
pub use qr::{
    Attempt, HostFacts, PairError, PairSession, Preflight, QrPairing, MAX_FAILURES, TOKEN_TTL_SECS,
};
pub use ssh::{SshPairResult, SshPairing};
