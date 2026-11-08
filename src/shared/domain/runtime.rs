use serde::Serialize;

/// Terminal multiplexer that owns an agent pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Herdr,
    Tmux,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Herdr => "herdr",
            Runtime::Tmux => "tmux",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "herdr" => Some(Runtime::Herdr),
            "tmux" => Some(Runtime::Tmux),
            _ => None,
        }
    }
}

/// Pane ids end up as argv entries; restrict them to a safe alphabet anyway.
pub fn valid_pane_id(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && !p.starts_with('-')
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":%._-@$".contains(&b))
}

/// Where a pane lives beyond its id, and who was in it when the event was
/// recorded. Every field is optional: older events and some sources lack them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneContext {
    /// tmux server socket (absolute path) or Herdr session name.
    pub session: Option<String>,
    /// tmux `#{pane_pid}`: the process the pane was started with.
    pub pane_pid: Option<u32>,
    /// Herdr `state_change_seq` of the blocked state the event belongs to.
    pub agent_seq: Option<u64>,
}

/// tmux socket paths and Herdr session names are argv entries too.
pub fn valid_session(runtime: Runtime, s: &str) -> bool {
    match runtime {
        Runtime::Tmux => {
            s.starts_with('/')
                && s.len() <= 512
                && !s.contains("/../")
                && s.chars().all(|c| !c.is_control() && c != ',')
        }
        Runtime::Herdr => {
            !s.is_empty()
                && s.len() <= 64
                && !s.starts_with('-')
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        }
    }
}

/// An agent pane reference (`w7:pA` for Herdr, `%3` for tmux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneRef {
    pub runtime: Runtime,
    pub pane: String,
    #[serde(skip)]
    pub context: PaneContext,
}

impl PaneRef {
    pub fn new(runtime: Runtime, pane: impl Into<String>) -> Self {
        Self {
            runtime,
            pane: pane.into(),
            context: PaneContext::default(),
        }
    }

    /// The validated session, if any (an invalid one is dropped, not used).
    pub fn session(&self) -> Option<&str> {
        self.context
            .session
            .as_deref()
            .filter(|s| valid_session(self.runtime, s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_restricted_per_runtime() {
        assert!(valid_session(Runtime::Tmux, "/tmp/tmux-1000/default"));
        for bad in ["default", "-S", "/tmp/a/../b", "/tmp/a\nb", "/tmp/a,1"] {
            assert!(!valid_session(Runtime::Tmux, bad), "{bad:?}");
        }
        assert!(valid_session(Runtime::Herdr, "work.2"));
        for bad in ["", "..", "a/b", "-x", "a b"] {
            assert!(!valid_session(Runtime::Herdr, bad), "{bad:?}");
        }
    }
}
