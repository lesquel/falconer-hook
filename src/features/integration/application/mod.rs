//! `integration install|uninstall|list`.

use serde::Serialize;

use crate::features::integration::domain::nested::Hook;
use crate::features::integration::domain::{
    agent_command, aider, flat, nested, owned, AgentSpec, Format, IntegrationError,
};

/// A configuration file on disk.
pub trait SettingsFile {
    /// `None` when the file does not exist.
    fn load(&self) -> Result<Option<String>, IntegrationError>;
    fn save(&self, contents: &str) -> Result<(), IntegrationError>;
    /// Deletes the file; a missing file is not an error.
    fn remove(&self) -> Result<(), IntegrationError>;
    fn location(&self) -> String;
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct IntegrationResult {
    pub agent: &'static str,
    pub settings: String,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

pub struct Integration<'a, F> {
    pub file: &'a F,
    pub spec: &'static AgentSpec,
    /// Absolute path of the `openmosh-hook` executable the hooks call.
    pub exe: String,
}

impl<F: SettingsFile> Integration<'_, F> {
    fn agent(&self) -> &'static str {
        self.spec.id.as_str()
    }

    fn result(&self, changed: bool, install: bool) -> IntegrationResult {
        IntegrationResult {
            agent: self.agent(),
            settings: self.file.location(),
            changed,
            note: if install && changed {
                self.spec.note
            } else {
                None
            },
        }
    }

    fn hooks(&self, events: &[(&'static str, Option<&'static str>)]) -> Vec<Hook<'static>> {
        events
            .iter()
            .map(|(event, kind)| Hook {
                event,
                command: agent_command(&self.exe, self.agent(), *kind),
            })
            .collect()
    }

    /// Parses the file as a JSON object; `None` when absent or blank.
    fn load_json(&self) -> Result<Option<serde_json::Value>, IntegrationError> {
        match self.file.load()? {
            Some(t) if !t.trim().is_empty() => serde_json::from_str(&t)
                .map(Some)
                .map_err(|_| IntegrationError::NotAnObject),
            _ => Ok(None),
        }
    }

    fn edit_json(
        &self,
        create: bool,
        f: impl FnOnce(&mut serde_json::Value) -> Result<bool, IntegrationError>,
    ) -> Result<bool, IntegrationError> {
        let mut v = match self.load_json()? {
            Some(v) => v,
            None if create => serde_json::json!({}),
            None => return Ok(false),
        };
        let changed = f(&mut v)?;
        if changed && !create && is_empty_config(&v) {
            // Nothing but our hooks was there: remove the file instead of leaving `{}`.
            self.file.remove()?;
            return Ok(true);
        }
        if changed {
            let mut out = serde_json::to_string_pretty(&v).map_err(|_| IntegrationError::Io)?;
            out.push('\n');
            self.file.save(&out)?;
        }
        Ok(changed)
    }

    pub fn install(&self) -> Result<IntegrationResult, IntegrationError> {
        let agent = self.agent();
        let changed = match self.spec.format {
            Format::Nested {
                events,
                matcher,
                name,
            } => {
                let hooks = self.hooks(events);
                self.edit_json(true, |v| nested::install(v, agent, &hooks, matcher, name))?
            }
            Format::Flat { events } => {
                let pairs: Vec<_> = events.iter().map(|(e, k)| (*e, Some(*k))).collect();
                let hooks = self.hooks(&pairs);
                self.edit_json(true, |v| flat::install(v, agent, &hooks))?
            }
            Format::Owned(kind) => {
                match owned::plan_install(kind, self.file.load()?.as_deref(), &self.exe)? {
                    Some(text) => {
                        self.file.save(&text)?;
                        true
                    }
                    None => false,
                }
            }
            Format::AiderBlock => match aider::install(self.file.load()?.as_deref(), &self.exe)? {
                Some(text) => {
                    self.file.save(&text)?;
                    true
                }
                None => false,
            },
        };
        Ok(self.result(changed, true))
    }

    pub fn uninstall(&self) -> Result<IntegrationResult, IntegrationError> {
        let agent = self.agent();
        let changed = match self.spec.format {
            Format::Nested { .. } => self.edit_json(false, |v| nested::uninstall(v, agent))?,
            Format::Flat { .. } => self.edit_json(false, |v| flat::uninstall(v, agent))?,
            Format::Owned(kind) => match self.file.load()? {
                Some(t) if owned::is_ours(kind, &t) => {
                    self.file.remove()?;
                    true
                }
                _ => false,
            },
            Format::AiderBlock => match self.file.load()? {
                Some(t) => match aider::uninstall(&t)? {
                    Some(rest) => {
                        self.file.save(&rest)?;
                        true
                    }
                    None => false,
                },
                None => false,
            },
        };
        Ok(self.result(changed, false))
    }

    /// Whether our hooks are present (read-only).
    pub fn installed(&self) -> Result<bool, IntegrationError> {
        let agent = self.agent();
        match self.spec.format {
            Format::Nested { .. } => self
                .load_json()?
                .map_or(Ok(false), |v| nested::installed(&v, agent)),
            Format::Flat { .. } => self
                .load_json()?
                .map_or(Ok(false), |v| flat::installed(&v, agent)),
            Format::Owned(kind) => Ok(self.file.load()?.is_some_and(|t| owned::is_ours(kind, &t))),
            Format::AiderBlock => self
                .file
                .load()?
                .map_or(Ok(false), |t| aider::installed(&t)),
        }
    }
}

/// A config object that holds nothing a person wrote: only empty objects/arrays and a
/// schema `version` number (Cursor and Copilot hook files carry one).
fn is_empty_config(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => m.iter().all(|(k, v)| {
            (k == "version" && v.is_number())
                || matches!(v, serde_json::Value::Object(o) if o.is_empty())
                || matches!(v, serde_json::Value::Array(a) if a.is_empty())
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::integration::domain::AgentId;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Mem(RefCell<Option<String>>, RefCell<u32>);
    impl SettingsFile for Mem {
        fn load(&self) -> Result<Option<String>, IntegrationError> {
            Ok(self.0.borrow().clone())
        }
        fn save(&self, c: &str) -> Result<(), IntegrationError> {
            *self.1.borrow_mut() += 1;
            *self.0.borrow_mut() = Some(c.into());
            Ok(())
        }
        fn remove(&self) -> Result<(), IntegrationError> {
            *self.0.borrow_mut() = None;
            Ok(())
        }
        fn location(&self) -> String {
            "mem".into()
        }
    }

    fn uc(f: &Mem, id: AgentId) -> Integration<'_, Mem> {
        Integration {
            file: f,
            spec: id.spec(),
            exe: "/x/openmosh-hook".into(),
        }
    }

    #[test]
    fn claude_install_creates_and_skips_rewrite_when_unchanged() {
        let f = Mem::default();
        let uc = uc(&f, AgentId::Claude);
        assert!(uc.install().unwrap().changed);
        assert!(!uc.install().unwrap().changed);
        assert_eq!(*f.1.borrow(), 1, "unchanged settings are not rewritten");
        assert!(uc.installed().unwrap());
        assert!(uc.uninstall().unwrap().changed);
        assert_eq!(
            f.0.borrow().as_deref(),
            None,
            "a file holding only our hooks is removed"
        );
        assert!(!uc.installed().unwrap());
    }

    #[test]
    fn every_agent_round_trips_on_an_existing_file() {
        let originals = [
            (AgentId::Claude, "{\"model\": \"opus\"}"),
            (AgentId::Codex, "{\"description\": \"mine\"}"),
            (AgentId::Gemini, "{\"general\": {\"vimMode\": true}}"),
            (
                AgentId::Cursor,
                "{\"version\": 1, \"hooks\": {\"stop\": [{\"command\": \"say hi\"}]}}",
            ),
            (AgentId::Aider, "model: sonnet\n"),
        ];
        for (id, original) in originals {
            let f = Mem(RefCell::new(Some(original.into())), RefCell::new(0));
            let uc = uc(&f, id);
            let r = uc.install().unwrap();
            assert!(r.changed, "{id:?}");
            assert!(uc.installed().unwrap(), "{id:?}");
            assert!(!uc.install().unwrap().changed, "{id:?} idempotent");
            assert!(uc.uninstall().unwrap().changed, "{id:?}");
            let now = f.0.borrow().clone().unwrap();
            if id == AgentId::Aider {
                assert_eq!(now, original);
            } else {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&now).unwrap(),
                    serde_json::from_str::<serde_json::Value>(original).unwrap(),
                    "{id:?}"
                );
            }
            assert!(!uc.uninstall().unwrap().changed, "{id:?}");
        }
    }

    #[test]
    fn owned_files_are_created_and_deleted_but_foreign_ones_kept() {
        for id in [AgentId::Copilot, AgentId::Kiro, AgentId::Opencode] {
            let f = Mem::default();
            let uc = uc(&f, id);
            let r = uc.install().unwrap();
            assert!(r.changed && r.note.is_some(), "{id:?}");
            assert!(!uc.install().unwrap().changed);
            assert!(uc.uninstall().unwrap().changed);
            assert!(f.0.borrow().is_none(), "{id:?} file removed");

            *f.0.borrow_mut() = Some("{\"version\": 1, \"hooks\": {}}\n".into());
            assert_eq!(uc.install().unwrap_err(), IntegrationError::Conflict);
            assert!(!uc.uninstall().unwrap().changed);
            assert!(f.0.borrow().is_some(), "{id:?} foreign file kept");
        }
    }

    #[test]
    fn uninstall_without_file_is_noop_and_invalid_json_errors() {
        let f = Mem::default();
        let uc = uc(&f, AgentId::Gemini);
        assert!(!uc.uninstall().unwrap().changed);
        assert!(f.0.borrow().is_none());
        assert!(!uc.installed().unwrap());
        *f.0.borrow_mut() = Some("{not json".into());
        assert_eq!(uc.install().unwrap_err(), IntegrationError::NotAnObject);
        assert_eq!(uc.installed().unwrap_err(), IntegrationError::NotAnObject);
        assert_eq!(*f.1.borrow(), 0, "malformed file never written");
    }

    #[test]
    fn uninstall_deletes_a_config_file_left_with_nothing_but_our_removed_hooks() {
        for id in [
            AgentId::Codex,
            AgentId::Cursor,
            AgentId::Gemini,
            AgentId::Claude,
        ] {
            let f = Mem::default();
            let uc = uc(&f, id);
            uc.install().unwrap();
            assert!(f.0.borrow().is_some());
            assert!(uc.uninstall().unwrap().changed);
            assert!(f.0.borrow().is_none(), "{id:?} left an empty config behind");
        }
    }

    #[test]
    fn uninstall_keeps_a_config_file_with_user_content() {
        let f = Mem::default();
        *f.0.borrow_mut() = Some("{\"keep\": true}".into());
        let uc = uc(&f, AgentId::Codex);
        uc.install().unwrap();
        uc.uninstall().unwrap();
        let left: serde_json::Value =
            serde_json::from_str(f.0.borrow().as_deref().unwrap()).unwrap();
        assert_eq!(left["keep"], true);
    }
}
