/// Source of the current time in unix seconds. Injected so use cases stay deterministic in tests.
pub trait Clock {
    fn now(&self) -> u64;
}

#[cfg(test)]
pub mod fakes {
    use super::Clock;
    use std::cell::Cell;

    /// A settable clock for tests.
    #[derive(Debug, Default)]
    pub struct FixedClock(pub Cell<u64>);

    impl FixedClock {
        pub fn at(t: u64) -> Self {
            Self(Cell::new(t))
        }
        pub fn advance(&self, secs: u64) {
            self.0.set(self.0.get() + secs);
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> u64 {
            self.0.get()
        }
    }
}
