//! Check results.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// What `doctor --fix <id>` can do about a check. Stable machine ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FixId {
    InstallRuntime,
    InstallTailscale,
    TailscaleUp,
    InstallSsh,
    EnableSsh,
    FixPermissions,
    HardenSsh,
    InstallService,
    /// Described only: pairing needs a person, so `--fix` never runs it.
    Pair,
}

impl FixId {
    pub const ALL: [FixId; 9] = [
        Self::InstallRuntime,
        Self::InstallTailscale,
        Self::TailscaleUp,
        Self::InstallSsh,
        Self::EnableSsh,
        Self::FixPermissions,
        Self::HardenSsh,
        Self::InstallService,
        Self::Pair,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InstallRuntime => "install_runtime",
            Self::InstallTailscale => "install_tailscale",
            Self::TailscaleUp => "tailscale_up",
            Self::InstallSsh => "install_ssh",
            Self::EnableSsh => "enable_ssh",
            Self::FixPermissions => "fix_permissions",
            Self::HardenSsh => "harden_ssh",
            Self::InstallService => "install_service",
            Self::Pair => "pair",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s)
    }

    /// What applying it does, for people.
    pub fn description(self) -> &'static str {
        match self {
            Self::InstallRuntime => {
                "install Herdr (Homebrew or its official installer) or tmux, after asking"
            }
            Self::InstallTailscale => {
                "install Tailscale (Homebrew or its official installer), after asking"
            }
            Self::TailscaleUp => "run `sudo tailscale up` and open the login URL it prints",
            Self::InstallSsh => {
                "install the OpenSSH server with the package manager, then turn it on"
            }
            Self::EnableSsh => "turn the SSH server on",
            Self::FixPermissions => {
                "make the config and state directories 0700 and their files 0600"
            }
            Self::HardenSsh => "turn off SSH password logins with `falconer-hook harden-ssh`",
            Self::InstallService => {
                "register the user service with `falconer-hook service install`"
            }
            Self::Pair => {
                "run `falconer-hook pair` and scan the QR code with Falconer (needs a person)"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub id: FixId,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// Present only when `doctor --fix` knows how to fix the problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

impl Check {
    pub fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail: detail.into(),
            fix: None,
        }
    }
    pub fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
            fix: None,
        }
    }
    pub fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
            fix: None,
        }
    }

    /// Attaches `id` to a warning or failure; a passing check stays as is.
    pub fn with_fix(mut self, id: FixId) -> Self {
        if self.status != Status::Ok {
            self.fix = Some(Fix {
                id,
                description: id.description().to_string(),
            });
        }
        self
    }

    /// The fix to apply: only for a warning or failure.
    pub fn fix_id(&self) -> Option<FixId> {
        self.fix
            .as_ref()
            .filter(|_| self.status != Status::Ok)
            .map(|f| f.id)
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
    fn fix_ids_round_trip_and_serialize_as_snake_case() {
        for id in FixId::ALL {
            assert_eq!(FixId::parse(id.as_str()), Some(id));
            assert_eq!(
                serde_json::to_value(id).unwrap(),
                serde_json::json!(id.as_str())
            );
            assert!(!id.description().is_empty());
        }
        assert_eq!(FixId::parse("bogus"), None);
    }

    #[test]
    fn fix_is_attached_only_to_problems_and_serialized_only_when_present() {
        let ok = Check::ok("a", "fine").with_fix(FixId::Pair);
        assert_eq!(ok.fix, None);
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            serde_json::json!({"name": "a", "status": "ok", "detail": "fine"})
        );
        let warn = Check::warn("devices", "none").with_fix(FixId::Pair);
        assert_eq!(warn.fix_id(), Some(FixId::Pair));
        let v = serde_json::to_value(&warn).unwrap();
        assert_eq!(v["fix"]["id"], "pair");
        assert_eq!(v["fix"]["description"], FixId::Pair.description());
    }

    #[test]
    fn healthy_unless_a_check_fails() {
        assert!(DoctorReport::new(vec![Check::ok("a", ""), Check::warn("b", "")]).healthy);
        assert!(!DoctorReport::new(vec![Check::ok("a", ""), Check::fail("b", "")]).healthy);
    }
}
