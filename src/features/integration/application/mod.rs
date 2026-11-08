//! `integration install|uninstall claude`.

use serde::Serialize;

use crate::features::integration::domain::{install, uninstall, IntegrationError};

/// A JSON settings document on disk.
pub trait SettingsFile {
    /// `None` when the file does not exist.
    fn load(&self) -> Result<Option<String>, IntegrationError>;
    fn save(&self, contents: &str) -> Result<(), IntegrationError>;
    fn location(&self) -> String;
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct IntegrationResult {
    pub agent: &'static str,
    pub settings: String,
    pub changed: bool,
}

pub struct ClaudeIntegration<'a, F> {
    pub file: &'a F,
    pub command: String,
}

impl<F: SettingsFile> ClaudeIntegration<'_, F> {
    fn edit(
        &self,
        f: impl FnOnce(&mut serde_json::Value) -> Result<bool, IntegrationError>,
        create: bool,
    ) -> Result<IntegrationResult, IntegrationError> {
        let text = self.file.load()?;
        let result = |changed| IntegrationResult {
            agent: "claude",
            settings: self.file.location(),
            changed,
        };
        let mut v = match text {
            Some(t) if !t.trim().is_empty() => {
                serde_json::from_str(&t).map_err(|_| IntegrationError::NotAnObject)?
            }
            _ if !create => return Ok(result(false)),
            _ => serde_json::json!({}),
        };
        let changed = f(&mut v)?;
        if changed {
            let mut out = serde_json::to_string_pretty(&v).map_err(|_| IntegrationError::Io)?;
            out.push('\n');
            self.file.save(&out)?;
        }
        Ok(result(changed))
    }

    pub fn install(&self) -> Result<IntegrationResult, IntegrationError> {
        let cmd = self.command.clone();
        self.edit(move |v| install(v, &cmd), true)
    }

    pub fn uninstall(&self) -> Result<IntegrationResult, IntegrationError> {
        self.edit(uninstall, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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
        fn location(&self) -> String {
            "mem".into()
        }
    }

    #[test]
    fn install_creates_and_skips_rewrite_when_unchanged() {
        let f = Mem(RefCell::new(None), RefCell::new(0));
        let uc = ClaudeIntegration {
            file: &f,
            command: "'/x/openmosh-hook' emit --source claude".into(),
        };
        assert!(uc.install().unwrap().changed);
        assert!(!uc.install().unwrap().changed);
        assert_eq!(*f.1.borrow(), 1, "unchanged settings are not rewritten");
        assert!(uc.uninstall().unwrap().changed);
        assert_eq!(f.0.borrow().as_deref(), Some("{}\n"));
    }

    #[test]
    fn uninstall_without_file_is_noop_and_invalid_json_errors() {
        let f = Mem(RefCell::new(None), RefCell::new(0));
        let uc = ClaudeIntegration {
            file: &f,
            command: String::new(),
        };
        assert!(!uc.uninstall().unwrap().changed);
        assert!(f.0.borrow().is_none());
        *f.0.borrow_mut() = Some("{not json".into());
        assert_eq!(uc.install().unwrap_err(), IntegrationError::NotAnObject);
    }
}
