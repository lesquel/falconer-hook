//! Check results.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail: detail.into(),
        }
    }
    pub fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
        }
    }
    pub fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// False when any check failed.
    pub healthy: bool,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn new(checks: Vec<Check>) -> Self {
        Self {
            healthy: checks.iter().all(|c| c.status != Status::Fail),
            checks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_unless_a_check_fails() {
        assert!(DoctorReport::new(vec![Check::ok("a", ""), Check::warn("b", "")]).healthy);
        assert!(!DoctorReport::new(vec![Check::ok("a", ""), Check::fail("b", "")]).healthy);
    }
}
