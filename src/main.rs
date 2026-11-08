//! `openmosh-hook` composition root.
#![forbid(unsafe_code)]
// Temporary while the CLI is being wired up; removed once every module is reachable.
#![allow(dead_code, unused_imports)]

mod features;
mod shared;
#[cfg(test)]
mod test_vectors;

fn main() {
    println!("openmosh-hook {}", env!("CARGO_PKG_VERSION"));
}
