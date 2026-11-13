//! `CommandRunner` adapter over `std::process::Command` (argv only, no shell).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::shared::domain::command::{CommandError, CommandOutput, CommandRunner};

pub struct ProcessRunner {
    pub timeout: Duration,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, CommandError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => CommandError::NotFound(program.to_string()),
                _ => CommandError::Spawn(program.to_string()),
            })?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let out_t = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = stdout.read_to_end(&mut s);
            s
        });
        let err_t = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = stderr.read_to_end(&mut s);
            s
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if start.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(CommandError::Timeout(program.to_string()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return Err(CommandError::Spawn(program.to_string())),
            }
        };
        let stdout = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
}

/// Runs `argv` attached to the user's terminal (stdin, stdout and stderr
/// inherited) so password prompts from `sudo` or `brew` work. Returns the
/// exit code. Used only for commands the user confirmed or passed `--yes` for.
pub fn run_inherited(argv: &[String]) -> Result<i32, CommandError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| CommandError::Spawn(String::new()))?;
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CommandError::NotFound(program.clone()),
            _ => CommandError::Spawn(program.clone()),
        })?;
    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_arguments_verbatim_without_a_shell() {
        let r = ProcessRunner::default();
        let out = r
            .run("printf", &["%s|".into(), "$(id) ; rm -rf /".into()])
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "$(id) ; rm -rf /|");
    }

    #[test]
    fn missing_program_is_not_found() {
        let r = ProcessRunner::default();
        assert!(matches!(
            r.run("openmosh-definitely-missing-binary", &[]),
            Err(CommandError::NotFound(_))
        ));
    }
}
