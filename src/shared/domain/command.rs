//! Port for running external programs. Every call is argv-based: there is no
//! shell and no string interpolation (control T-9).

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CommandError {
    #[error("program not found: {0}")]
    NotFound(String),
    #[error("failed to run {0}")]
    Spawn(String),
    #[error("{0} timed out")]
    Timeout(String),
}

/// Runs `program` with `args` as separate argv entries and captures its output.
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, CommandError>;
}

impl<T: CommandRunner + ?Sized> CommandRunner for &T {
    fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, CommandError> {
        (**self).run(program, args)
    }
}

#[cfg(test)]
pub mod fakes {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Records every argv it receives and replays scripted outputs in order.
    #[derive(Default)]
    pub struct RecordingRunner {
        pub calls: RefCell<Vec<Vec<String>>>,
        pub replies: RefCell<VecDeque<Result<CommandOutput, CommandError>>>,
    }

    impl RecordingRunner {
        pub fn reply_ok(&self, stdout: &str) -> &Self {
            self.replies.borrow_mut().push_back(Ok(CommandOutput {
                status: 0,
                stdout: stdout.to_string(),
                stderr: String::new(),
            }));
            self
        }
        pub fn reply(&self, r: Result<CommandOutput, CommandError>) -> &Self {
            self.replies.borrow_mut().push_back(r);
            self
        }
        pub fn argv(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl CommandRunner for RecordingRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, CommandError> {
            let mut argv = vec![program.to_string()];
            argv.extend(args.iter().cloned());
            self.calls.borrow_mut().push(argv);
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(CommandOutput {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }))
        }
    }
}
