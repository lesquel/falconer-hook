//! Apply a service plan through ports (or only describe it with `--dry-run`).

use crate::features::service::domain::{ServiceError, ServicePlan};
use crate::shared::domain::command::CommandRunner;

/// Writes and removes service files.
pub trait ServiceFiles {
    fn write(&self, path: &std::path::Path, contents: &str) -> Result<(), ServiceError>;
    fn remove(&self, path: &std::path::Path) -> Result<(), ServiceError>;
}

pub struct ApplyPlan<'a, F> {
    pub files: &'a F,
    pub runner: &'a dyn CommandRunner,
}

impl<F: ServiceFiles> ApplyPlan<'_, F> {
    /// Files first, then commands in order; stops at the first failing
    /// command. On uninstall, `disable` failing (unit not loaded) is tolerated.
    pub fn apply(&self, plan: &ServicePlan) -> Result<(), ServiceError> {
        for (p, c) in &plan.write {
            self.files.write(p, c)?;
        }
        for p in &plan.remove {
            self.files.remove(p)?;
        }
        let uninstall = !plan.remove.is_empty();
        for cmd in &plan.commands {
            let (prog, args) = cmd.split_first().ok_or(ServiceError::Io)?;
            let out = self
                .runner
                .run(prog, args)
                .map_err(|e| ServiceError::Command(e.to_string()))?;
            if !out.success() && !uninstall {
                return Err(ServiceError::Command(args.join(" ")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::service::domain::{install_plan, uninstall_plan, Platform};
    use crate::shared::domain::command::fakes::RecordingRunner;
    use crate::shared::domain::command::CommandOutput;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    #[derive(Default)]
    struct MemFiles(RefCell<Vec<String>>);
    impl ServiceFiles for MemFiles {
        fn write(&self, p: &Path, _: &str) -> Result<(), ServiceError> {
            self.0.borrow_mut().push(format!("write {}", p.display()));
            Ok(())
        }
        fn remove(&self, p: &Path) -> Result<(), ServiceError> {
            self.0.borrow_mut().push(format!("remove {}", p.display()));
            Ok(())
        }
    }

    #[test]
    fn install_writes_then_runs_commands_and_stops_on_failure() {
        let files = MemFiles::default();
        let runner = RecordingRunner::default();
        let unit = PathBuf::from("/h/u.service");
        let plan = install_plan(
            Platform::Systemd,
            Path::new("/h/x"),
            Path::new("/h"),
            &unit,
            "systemctl",
            "launchctl",
        )
        .unwrap();
        ApplyPlan {
            files: &files,
            runner: &runner,
        }
        .apply(&plan)
        .unwrap();
        assert_eq!(files.0.borrow().as_slice(), ["write /h/u.service"]);
        assert_eq!(runner.argv().len(), 2);

        let runner = RecordingRunner::default();
        runner.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: String::new(),
        }));
        assert!(ApplyPlan {
            files: &files,
            runner: &runner
        }
        .apply(&plan)
        .is_err());
        assert_eq!(runner.argv().len(), 1);
    }

    #[test]
    fn uninstall_tolerates_unit_not_loaded() {
        let files = MemFiles::default();
        let runner = RecordingRunner::default();
        runner.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: String::new(),
        }));
        let plan = uninstall_plan(
            Platform::Systemd,
            Path::new("/h/u.service"),
            "systemctl",
            "launchctl",
        )
        .unwrap();
        ApplyPlan {
            files: &files,
            runner: &runner,
        }
        .apply(&plan)
        .unwrap();
        assert_eq!(runner.argv().len(), 2);
    }
}
