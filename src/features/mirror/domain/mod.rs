//! Focus targets, socket events and the ports the mirror loop depends on.

use std::time::Duration;
use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

/// What a focus change points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusKind {
    Workspace,
    Tab,
    Pane,
}

impl FocusKind {
    /// Application order: a workspace before its tab before its pane.
    pub const ALL: [FocusKind; 3] = [FocusKind::Workspace, FocusKind::Tab, FocusKind::Pane];

    pub fn index(self) -> usize {
        match self {
            FocusKind::Workspace => 0,
            FocusKind::Tab => 1,
            FocusKind::Pane => 2,
        }
    }
}

/// A public Herdr id (`w1`, `w1:t2`, `w1:p3`) of a given kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusTarget {
    pub kind: FocusKind,
    pub id: String,
}

impl FocusTarget {
    /// Builds a target, refusing ids that are not plain public Herdr ids.
    pub fn new(kind: FocusKind, id: &str) -> Option<Self> {
        valid_target_id(id).then(|| Self {
            kind,
            id: id.to_string(),
        })
    }
}

/// Public Herdr ids are short ASCII tokens such as `w1:p2`.
pub fn valid_target_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":_-".contains(&b))
}

/// Herdr session names map to a directory, so keep them to a safe charset.
pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Something the subscription delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketEvent {
    /// A client (any client) focused this target.
    Focused(FocusTarget),
    /// The server dropped events for this subscriber; local state is stale.
    EventsLost,
}

/// The server's current focus, used as the baseline after (re)connecting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusSnapshot {
    pub workspace: Option<String>,
    pub tab: Option<String>,
    pub pane: Option<String>,
}

impl FocusSnapshot {
    pub fn get(&self, kind: FocusKind) -> Option<&str> {
        match kind {
            FocusKind::Workspace => self.workspace.as_deref(),
            FocusKind::Tab => self.tab.as_deref(),
            FocusKind::Pane => self.pane.as_deref(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MirrorError {
    #[error("herdr socket unavailable")]
    Unavailable,
    #[error("herdr socket closed")]
    Closed,
    #[error("herdr protocol error: {0}")]
    Protocol(&'static str),
    #[error("failed to start {0}")]
    Launch(String),
}

impl ErrorCode for MirrorError {
    fn code(&self) -> &'static str {
        match self {
            MirrorError::Unavailable | MirrorError::Closed => "runtime_unavailable",
            MirrorError::Protocol(_) => "runtime_error",
            MirrorError::Launch(_) => "runtime_unavailable",
        }
    }
}

/// Herdr's event and command socket.
pub trait FocusSocket {
    /// Opens (or reopens) the focus subscription and the command connection.
    fn connect(&mut self) -> Result<(), MirrorError>;
    /// Waits up to `wait` for the next relevant event; `Ok(None)` on timeout.
    fn next_event(&mut self, wait: Duration) -> Result<Option<SocketEvent>, MirrorError>;
    /// Reads the server's current focus.
    fn snapshot(&mut self) -> Result<FocusSnapshot, MirrorError>;
    /// Focuses `target` for every attached client.
    fn focus(&mut self, target: &FocusTarget) -> Result<(), MirrorError>;
}

/// Monotonic time in milliseconds plus sleeping, injected for deterministic tests.
pub trait MonoClock {
    fn now_ms(&self) -> u64;
    fn sleep(&self, d: Duration);
}

/// A running child process.
pub trait ChildProcess {
    /// Waits for the child and returns its exit code (128 + signal when killed).
    fn wait(&mut self) -> Result<i32, MirrorError>;
}

/// Starts a program with the inherited terminal (argv only, no shell).
pub trait Launcher {
    type Child: ChildProcess;
    fn spawn(&self, program: &str, args: &[String]) -> Result<Self::Child, MirrorError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_ids_are_plain_public_ids() {
        assert!(FocusTarget::new(FocusKind::Pane, "w1:p2").is_some());
        assert!(FocusTarget::new(FocusKind::Tab, "w_a-1:t3").is_some());
        assert!(FocusTarget::new(FocusKind::Tab, "").is_none());
        assert!(FocusTarget::new(FocusKind::Tab, "w1 t2").is_none());
        assert!(FocusTarget::new(FocusKind::Tab, "w1\"}").is_none());
        assert!(FocusTarget::new(FocusKind::Tab, &"a".repeat(65)).is_none());
    }

    #[test]
    fn session_names_cannot_escape_the_sessions_dir() {
        assert!(valid_session_name("work"));
        assert!(valid_session_name("side-project.2"));
        for bad in ["", ".", "..", "a/b", "../x", "a b", "-x", "--help"] {
            assert!(!valid_session_name(bad), "{bad:?}");
        }
    }
}
