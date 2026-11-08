//! System clock and OS-random id generator.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::shared::domain::clock::Clock;
use crate::shared::domain::encoding::b64u_encode;
use crate::shared::domain::ids::IdGenerator;
use crate::shared::infrastructure::crypto::random_bytes;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// 16 random bytes from the OS CSPRNG, b64u encoded.
pub struct OsIds;

impl IdGenerator for OsIds {
    fn new_id(&self) -> String {
        b64u_encode(&random_bytes::<16>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::ids::is_valid_id;

    #[test]
    fn os_ids_are_valid_and_distinct() {
        let a = OsIds.new_id();
        let b = OsIds.new_id();
        assert!(is_valid_id(&a));
        assert_ne!(a, b);
    }
}
