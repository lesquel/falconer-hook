//! Executes an uninstall plan step by step. A failed step does not stop
//! the others (so a broken agent file never leaves the service running);
//! each outcome is reported.

use serde::Serialize;

use crate::features::setup::domain::Step;

/// The side effects of each step, implemented by the composition root.
pub trait UninstallPorts {
    fn service(&self, unit: &std::path::Path) -> Result<(), String>;
    fn integration(&self, agent: &str) -> Result<(), String>;
    fn authorized_keys(&self) -> Result<usize, String>;
    fn remove_dir(&self, path: &std::path::Path) -> Result<(), String>;
    fn remove_binary(&self, path: &std::path::Path) -> Result<(), String>;
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Outcome {
    #[serde(flatten)]
    pub step: Step,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn execute(ports: &dyn UninstallPorts, steps: Vec<Step>) -> Vec<Outcome> {
    steps
        .into_iter()
        .map(|step| {
            let r = match &step {
                Step::Service { unit } => ports.service(unit),
                Step::Integration { agent, .. } => ports.integration(agent),
                Step::AuthorizedKeys { .. } => ports.authorized_keys().map(|_| ()),
                Step::RemoveDir { path } => ports.remove_dir(path),
                Step::RemoveBinary { path } => ports.remove_binary(path),
            };
            Outcome {
                step,
                ok: r.is_ok(),
                error: r.err(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::Path;

    #[derive(Default)]
    struct Log(RefCell<Vec<String>>);
    impl UninstallPorts for Log {
        fn service(&self, _: &Path) -> Result<(), String> {
            self.0.borrow_mut().push("service".into());
            Ok(())
        }
        fn integration(&self, agent: &str) -> Result<(), String> {
            self.0.borrow_mut().push(agent.into());
            Err("bad file".into())
        }
        fn authorized_keys(&self) -> Result<usize, String> {
            self.0.borrow_mut().push("keys".into());
            Ok(1)
        }
        fn remove_dir(&self, p: &Path) -> Result<(), String> {
            self.0.borrow_mut().push(p.display().to_string());
            Ok(())
        }
        fn remove_binary(&self, _: &Path) -> Result<(), String> {
            self.0.borrow_mut().push("binary".into());
            Ok(())
        }
    }

    #[test]
    fn a_failed_step_does_not_stop_the_rest() {
        let log = Log::default();
        let out = execute(
            &log,
            vec![
                Step::Service { unit: "/u".into() },
                Step::Integration {
                    agent: "gemini",
                    settings: "/g".into(),
                },
                Step::AuthorizedKeys {
                    path: "/k".into(),
                    lines: 1,
                },
                Step::RemoveDir {
                    path: "/s/openmosh".into(),
                },
                Step::RemoveBinary { path: "/b".into() },
            ],
        );
        assert_eq!(
            *log.0.borrow(),
            ["service", "gemini", "keys", "/s/openmosh", "binary"]
        );
        assert!(!out[1].ok);
        assert_eq!(out[1].error.as_deref(), Some("bad file"));
        assert!(out.iter().enumerate().all(|(i, o)| i == 1 || o.ok));
        let v = serde_json::to_value(&out[1]).unwrap();
        assert_eq!(v["step"], "integration");
        assert_eq!(v["agent"], "gemini");
    }
}
