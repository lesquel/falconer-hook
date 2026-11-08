/// Generates opaque random identifiers (16 random bytes, b64u).
pub trait IdGenerator {
    fn new_id(&self) -> String;
}

/// True when `s` looks like an id this crate generates: 22 b64u characters.
pub fn is_valid_id(s: &str) -> bool {
    s.len() == 22
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
pub mod fakes {
    use super::IdGenerator;
    use std::cell::Cell;

    /// Deterministic ids: `AAAAAAAAAAAAAAAAAAAA01`, `...02`, …
    #[derive(Default)]
    pub struct SeqIds(pub Cell<u32>);

    impl IdGenerator for SeqIds {
        fn new_id(&self) -> String {
            let n = self.0.get() + 1;
            self.0.set(n);
            format!("AAAAAAAAAAAAAAAAAAAA{n:02}")
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn id_shape() {
        assert!(super::is_valid_id("AAAAAAAAAAAAAAAAAAAA01"));
        assert!(!super::is_valid_id("short"));
        assert!(!super::is_valid_id("AAAAAAAAAAAAAAAAAAAA0/"));
    }
}
