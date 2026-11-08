//! Pure edits of agent configuration files so each agent's own hook (or
//! plugin, or notification command) runs `openmosh-hook emit --source <agent>`.
//!
//! Every edit preserves unrelated content, is idempotent, repoints our
//! commands when the executable moved, and refuses unexpected shapes
//! instead of guessing. A command is "ours" when it names `openmosh-hook`
//! and contains ` emit --source <agent>`.

pub mod aider;
pub mod catalog;
pub mod flat;
pub mod nested;
pub mod owned;

use thiserror::Error;

use crate::shared::domain::error::ErrorCode;

pub use catalog::{AgentId, AgentSpec, Base, Format, CATALOG};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrationError {
    #[error("settings file is not a JSON object")]
    NotAnObject,
    #[error("settings file has an unexpected `hooks` shape; edit it manually")]
    BadHooks,
    #[error("the file exists and was not created by openmosh-hook; edit it manually")]
    Conflict,
    #[error("settings file could not be read or written")]
    Io,
}

impl ErrorCode for IntegrationError {
    fn code(&self) -> &'static str {
        match self {
            IntegrationError::NotAnObject | IntegrationError::BadHooks => "settings_invalid",
            IntegrationError::Conflict => "settings_conflict",
            IntegrationError::Io => "io_error",
        }
    }
}

/// Single-quotes `s` for a POSIX shell (agents run hook commands via a shell).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn marker(agent: &str) -> String {
    format!(" emit --source {agent}")
}

/// `'<exe>' emit --source <agent>[ --kind <kind>]`.
pub fn agent_command(exe: &str, agent: &str, kind: Option<&str>) -> String {
    let mut c = format!("{}{}", shell_quote(exe), marker(agent));
    if let Some(k) = kind {
        c.push_str(" --kind ");
        c.push_str(k);
    }
    c
}

/// Whether `command` is one of our hook commands for `agent`.
pub fn is_our_command(command: &str, agent: &str) -> bool {
    let m = marker(agent);
    command.contains("openmosh-hook")
        && (command.ends_with(&m) || command.contains(&format!("{m} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_shell_quoted() {
        assert_eq!(
            agent_command("/opt/it's/openmosh-hook", "claude", None),
            r"'/opt/it'\''s/openmosh-hook' emit --source claude"
        );
        assert_eq!(
            agent_command("/b/openmosh-hook", "codex", Some("done")),
            "'/b/openmosh-hook' emit --source codex --kind done"
        );
    }

    #[test]
    fn ours_matches_the_agent_exactly() {
        let c = agent_command("/b/openmosh-hook", "codex", Some("done"));
        assert!(is_our_command(&c, "codex"));
        assert!(!is_our_command(&c, "cod"));
        assert!(is_our_command(
            "'/b/openmosh-hook' emit --source claude",
            "claude"
        ));
        assert!(!is_our_command(
            "notify-send emit --source claude",
            "claude"
        ));
        assert!(!is_our_command(
            "'/b/openmosh-hook' emit --source claudex",
            "claude"
        ));
    }
}
