//! `AgentRuntime` adapters over the `herdr` and `tmux` CLIs (argv only).

pub mod herdr;
pub mod tmux;

use crate::features::agents::domain::AgentError;
use crate::shared::domain::command::{CommandError, CommandOutput, CommandRunner};

fn run(
    runner: &dyn CommandRunner,
    name: &'static str,
    program: &str,
    args: &[&str],
) -> Result<CommandOutput, AgentError> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match runner.run(program, &args) {
        Ok(o) => Ok(o),
        Err(CommandError::NotFound(_)) => Err(AgentError::Unavailable(name)),
        Err(_) => Err(AgentError::Failed(name)),
    }
}
