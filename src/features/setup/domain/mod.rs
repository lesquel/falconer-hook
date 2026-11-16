//! The uninstall plan. Only things falconer-hook created are ever listed:
//! its user service, its own hooks, `authorized_keys` lines tagged
//! `openmosh:`, its `falconer` config/state directories and, with `--all`,
//! the `falconer-hook` binary.

pub mod prereqs;

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Stop, disable and remove the user service.
    Service { unit: PathBuf },
    /// Remove our hooks from an agent's file.
    Integration {
        agent: &'static str,
        settings: String,
    },
    /// Remove the lines tagged `openmosh:` (paired phones).
    AuthorizedKeys { path: PathBuf, lines: usize },
    /// Delete an `falconer` directory (config, or state with devices and events).
    RemoveDir { path: PathBuf },
    /// Delete the `falconer-hook` binary.
    RemoveBinary { path: PathBuf },
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::Service { unit } => format!("remove the user service {}", unit.display()),
            Step::Integration { agent, settings } => {
                format!("remove the {agent} hooks from {settings}")
            }
            Step::AuthorizedKeys { path, lines } => format!(
                "remove {lines} paired-phone key line(s) tagged falconer: from {}",
                path.display()
            ),
            Step::RemoveDir { path } => format!("delete {}", path.display()),
            Step::RemoveBinary { path } => format!("delete the binary {}", path.display()),
        }
    }
}

/// What exists on this machine, gathered read-only.
#[derive(Debug, Default)]
pub struct Facts {
    /// The service file, when it exists.
    pub unit: Option<PathBuf>,
    /// Agents whose file holds our hooks.
    pub integrations: Vec<(&'static str, String)>,
    pub authorized_keys: PathBuf,
    /// Lines tagged `openmosh:` in `authorized_keys`.
    pub tagged_keys: usize,
    /// Existing config/state directories.
    pub dirs: Vec<PathBuf>,
    pub binary: PathBuf,
}

/// Only directories named `falconer` are ever deleted.
pub fn deletable_dir(p: &Path) -> bool {
    p.is_absolute() && p.file_name().is_some_and(|n| n == "falconer")
}

/// Only a file named `falconer-hook` is ever deleted.
pub fn deletable_binary(p: &Path) -> bool {
    p.is_absolute() && p.file_name().is_some_and(|n| n == "falconer-hook")
}

/// Ordered plan: stop the service first, delete the binary last.
pub fn plan_uninstall(f: Facts, all: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(unit) = f.unit {
        steps.push(Step::Service { unit });
    }
    for (agent, settings) in f.integrations {
        steps.push(Step::Integration { agent, settings });
    }
    if f.tagged_keys > 0 {
        steps.push(Step::AuthorizedKeys {
            path: f.authorized_keys,
            lines: f.tagged_keys,
        });
    }
    for d in f.dirs.into_iter().filter(|d| deletable_dir(d)) {
        steps.push(Step::RemoveDir { path: d });
    }
    if all && deletable_binary(&f.binary) {
        steps.push(Step::RemoveBinary { path: f.binary });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            unit: Some("/h/.config/systemd/user/falconer-hook.service".into()),
            integrations: vec![("claude", "/h/.claude/settings.json".into())],
            authorized_keys: "/h/.ssh/authorized_keys".into(),
            tagged_keys: 2,
            dirs: vec![
                "/h/.config/falconer".into(),
                "/h/.local/state/falconer".into(),
                "/h/.config".into(),
            ],
            binary: "/h/.local/bin/falconer-hook".into(),
        }
    }

    #[test]
    fn plan_is_ordered_and_binary_needs_all() {
        let steps = plan_uninstall(facts(), false);
        assert!(matches!(steps[0], Step::Service { .. }));
        assert!(matches!(
            steps[1],
            Step::Integration {
                agent: "claude",
                ..
            }
        ));
        assert!(matches!(steps[2], Step::AuthorizedKeys { lines: 2, .. }));
        assert_eq!(steps.len(), 5, "the non-falconer dir is never listed");
        assert!(!steps.iter().any(|s| matches!(s, Step::RemoveBinary { .. })));
        let all = plan_uninstall(facts(), true);
        assert_eq!(
            all.last(),
            Some(&Step::RemoveBinary {
                path: "/h/.local/bin/falconer-hook".into()
            })
        );
    }

    #[test]
    fn nothing_installed_means_an_empty_plan_and_foreign_binaries_are_kept() {
        let f = Facts {
            binary: "/usr/bin/cargo".into(),
            ..Facts::default()
        };
        assert!(plan_uninstall(f, true).is_empty());
        assert!(!deletable_dir(Path::new("falconer")));
        assert!(!deletable_binary(Path::new("/x/falconer-hook-old")));
    }
}
