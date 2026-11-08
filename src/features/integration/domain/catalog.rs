//! The agents openmosh-hook can wire, where each keeps its hook config, and
//! which of its events map to which OpenMosh event kind. Sources are the
//! agents' official docs (see `doc` on each entry).

use serde::Serialize;

use super::owned::OwnedKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    Claude,
    Codex,
    Gemini,
    Cursor,
    Copilot,
    Kiro,
    Opencode,
    Aider,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentId::Claude => "claude",
            AgentId::Codex => "codex",
            AgentId::Gemini => "gemini",
            AgentId::Cursor => "cursor",
            AgentId::Copilot => "copilot",
            AgentId::Kiro => "kiro",
            AgentId::Opencode => "opencode",
            AgentId::Aider => "aider",
        }
    }

    pub fn spec(self) -> &'static AgentSpec {
        CATALOG
            .iter()
            .find(|s| s.id == self)
            .expect("every agent has a catalog entry")
    }
}

/// Directory a path is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Home,
    /// `$XDG_CONFIG_HOME` (default `~/.config`).
    XdgConfig,
}

/// How the agent's file is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `hooks.<Event>: [{matcher?, hooks: [{type: "command", command, name?}]}]`.
    /// `events` pairs an agent event with the `--kind` passed to `emit`.
    Nested {
        events: &'static [(&'static str, Option<&'static str>)],
        matcher: Option<&'static str>,
        name: Option<&'static str>,
    },
    /// `{version: 1, hooks: {<event>: [{command}]}}`.
    Flat {
        events: &'static [(&'static str, &'static str)],
    },
    /// A whole file created (and removed) by openmosh-hook.
    Owned(OwnedKind),
    /// A marked block of top-level keys in a YAML file.
    AiderBlock,
}

#[derive(Debug)]
pub struct AgentSpec {
    pub id: AgentId,
    pub name: &'static str,
    /// Default file edited by `integration install`.
    pub base: Base,
    pub rel: &'static str,
    /// Executables whose presence on `PATH` means the agent is installed.
    pub bins: &'static [&'static str],
    /// Paths whose existence means the agent is installed.
    pub detect: &'static [(Base, &'static str)],
    pub format: Format,
    pub doc: &'static str,
    /// Shown after a successful install.
    pub note: Option<&'static str>,
}

pub const CATALOG: &[AgentSpec] = &[
    AgentSpec {
        id: AgentId::Claude,
        name: "Claude Code",
        base: Base::Home,
        rel: ".claude/settings.json",
        bins: &["claude"],
        detect: &[(Base::Home, ".claude")],
        format: Format::Nested {
            events: &[("Notification", None), ("Stop", None)],
            matcher: None,
            name: None,
        },
        doc: "https://code.claude.com/docs/en/hooks",
        note: None,
    },
    AgentSpec {
        id: AgentId::Codex,
        name: "OpenAI Codex CLI",
        base: Base::Home,
        rel: ".codex/hooks.json",
        bins: &["codex"],
        detect: &[(Base::Home, ".codex")],
        format: Format::Nested {
            events: &[
                ("PermissionRequest", Some("approval_required")),
                ("Stop", Some("done")),
            ],
            matcher: None,
            name: None,
        },
        doc: "https://learn.chatgpt.com/docs/hooks",
        note: Some("Codex asks you to review and trust new hooks the next time it starts."),
    },
    AgentSpec {
        id: AgentId::Gemini,
        name: "Gemini CLI",
        base: Base::Home,
        rel: ".gemini/settings.json",
        bins: &["gemini"],
        detect: &[(Base::Home, ".gemini")],
        format: Format::Nested {
            events: &[
                ("Notification", Some("notification")),
                ("AfterAgent", Some("done")),
            ],
            matcher: Some("*"),
            name: Some("openmosh"),
        },
        doc: "https://geminicli.com/docs/hooks/",
        note: None,
    },
    AgentSpec {
        id: AgentId::Cursor,
        name: "Cursor",
        base: Base::Home,
        rel: ".cursor/hooks.json",
        bins: &["cursor-agent", "cursor"],
        detect: &[(Base::Home, ".cursor")],
        format: Format::Flat {
            events: &[("stop", "done")],
        },
        doc: "https://cursor.com/docs/agent/hooks",
        note: None,
    },
    AgentSpec {
        id: AgentId::Copilot,
        name: "GitHub Copilot CLI",
        base: Base::Home,
        rel: ".copilot/hooks/openmosh.json",
        bins: &["copilot"],
        detect: &[(Base::Home, ".copilot")],
        format: Format::Owned(OwnedKind::Copilot),
        doc: "https://docs.github.com/en/copilot/reference/hooks-reference",
        note: Some("Copilot CLI loads hooks when it starts; restart running sessions."),
    },
    AgentSpec {
        id: AgentId::Kiro,
        name: "Kiro CLI",
        base: Base::Home,
        rel: ".kiro/hooks/openmosh.json",
        bins: &["kiro-cli"],
        detect: &[(Base::Home, ".kiro")],
        format: Format::Owned(OwnedKind::Kiro),
        doc: "https://kiro.dev/docs/hooks/",
        note: Some("Kiro CLI V3 loads global hooks when a session starts."),
    },
    AgentSpec {
        id: AgentId::Opencode,
        name: "OpenCode",
        base: Base::XdgConfig,
        rel: "opencode/plugins/openmosh.js",
        bins: &["opencode"],
        detect: &[(Base::XdgConfig, "opencode")],
        format: Format::Owned(OwnedKind::Opencode),
        doc: "https://opencode.ai/docs/plugins/",
        note: Some("OpenCode loads plugins when it starts; restart running sessions."),
    },
    AgentSpec {
        id: AgentId::Aider,
        name: "Aider",
        base: Base::Home,
        rel: ".aider.conf.yml",
        bins: &["aider"],
        detect: &[(Base::Home, ".aider.conf.yml")],
        format: Format::AiderBlock,
        doc: "https://aider.chat/docs/usage/notifications.html",
        note: None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_one_entry_with_a_doc_link() {
        for s in CATALOG {
            assert_eq!(s.id.spec().name, s.name);
            assert!(s.doc.starts_with("https://"), "{}", s.name);
            assert!(!s.bins.is_empty() && !s.detect.is_empty());
        }
        let ids: std::collections::HashSet<_> = CATALOG.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), CATALOG.len());
    }
}
