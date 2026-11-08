//! Pairing use cases.

mod devices;
mod qr;
mod ssh;

pub use devices::DeviceAdmin;
pub use qr::{Attempt, HostFacts, PairError, PairSession, Preflight, QrPairing};
pub use ssh::SshPairing;
