//! Keymaps embedded at build time from `data/keymaps/*.toml`.

use serde::Deserialize;

use crate::features::responses::domain::{Keymap, KeymapSource};

const EMBEDDED: &[&str] = &[
    include_str!("../../../../data/keymaps/claude.toml"),
    include_str!("../../../../data/keymaps/codex.toml"),
    include_str!("../../../../data/keymaps/opencode.toml"),
    include_str!("../../../../data/keymaps/cursor.toml"),
    include_str!("../../../../data/keymaps/kiro.toml"),
    include_str!("../../../../data/keymaps/gemini.toml"),
    include_str!("../../../../data/keymaps/copilot.toml"),
    include_str!("../../../../data/keymaps/aider.toml"),
    include_str!("../../../../data/keymaps/amp.toml"),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    agent: String,
    version: u32,
    verified: bool,
    allow: Vec<String>,
    deny: Vec<String>,
    #[serde(default)]
    submit: Vec<String>,
}

pub fn parse_keymap(text: &str) -> Option<Keymap> {
    let r: Raw = toml::from_str(text).ok()?;
    let k = Keymap {
        agent: r.agent,
        version: r.version,
        verified: r.verified,
        allow: r.allow,
        deny: r.deny,
        submit: r.submit,
    };
    k.is_valid().then_some(k)
}

pub struct EmbeddedKeymaps {
    maps: Vec<Keymap>,
}

impl EmbeddedKeymaps {
    pub fn load() -> Self {
        Self {
            maps: EMBEDDED.iter().filter_map(|t| parse_keymap(t)).collect(),
        }
    }

    pub fn all(&self) -> &[Keymap] {
        &self.maps
    }
}

impl KeymapSource for EmbeddedKeymaps {
    fn keymap(&self, agent: &str) -> Option<Keymap> {
        self.maps.iter().find(|k| k.agent == agent).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_keymap_parses() {
        let k = EmbeddedKeymaps::load();
        assert_eq!(k.all().len(), EMBEDDED.len());
        let claude = k.keymap("claude").unwrap();
        assert_eq!(claude.allow, vec!["enter"]);
        assert_eq!(claude.deny, vec!["esc"]);
        assert!(
            !claude.verified,
            "claude keys still need field verification"
        );
        assert!(k.keymap("unknown").is_none());
        for agent in [
            "codex", "opencode", "cursor", "kiro", "gemini", "copilot", "aider", "amp",
        ] {
            let m = k.keymap(agent).unwrap_or_else(|| panic!("{agent} keymap"));
            assert!(
                !m.verified,
                "{agent} keys are unverified until field-tested"
            );
        }
    }

    #[test]
    fn invalid_keymap_is_dropped() {
        assert!(
            parse_keymap("agent='x'\nversion=1\nverified=true\nallow=['C-c']\ndeny=['esc']\n")
                .is_none()
        );
        assert!(parse_keymap(
            "agent='x'\nversion=1\nverified=true\nallow=['y']\ndeny=['n']\nextra=1\n"
        )
        .is_none());
    }
}
