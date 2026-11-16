//! `falconer-hook` entry point. With `app/`, this is the composition root:
//! the only place that knows concrete adapter types.
#![forbid(unsafe_code)]

mod app;
mod cli;
mod features;
mod shared;
#[cfg(test)]
mod test_vectors;

use clap::{CommandFactory, Parser};
use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use cli::output::{err_envelope, ok_envelope};
use cli::{AttachCmd, Cli, Command};

fn main() -> ExitCode {
    let command = match Cli::parse().command {
        Some(c) => c,
        None if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() => Command::Tui,
        None => {
            // Not a terminal: the usual "missing command" help and exit code 2.
            let _ = Cli::command()
                .subcommand_required(true)
                .arg_required_else_help(true)
                .get_matches();
            return ExitCode::from(2);
        }
    };
    if let Command::Tui = command {
        return match app::Ctx::from_env().and_then(|ctx| app::run_tui(&ctx)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("falconer-hook: {}", e.message);
                ExitCode::from(1)
            }
        };
    }
    if let Command::Attach {
        cmd: AttachCmd::Herdr { session },
    } = &command
    {
        // The child owns the terminal; exit with its status code.
        return match app::Ctx::from_env()
            .and_then(|ctx| app::attach_herdr(&ctx, session.as_deref()))
        {
            Ok(code) => ExitCode::from(code),
            Err(e) => {
                eprintln!("falconer-hook: {}", e.message);
                ExitCode::from(1)
            }
        };
    }
    let json = command.json();
    let result = app::Ctx::from_env().and_then(|ctx| {
        if matches!(command, Command::Serve | Command::Pair { .. }) {
            let stop = ctx.stop.clone();
            let main = std::thread::current();
            // SIGINT/SIGTERM: stop the loop so the pairing token is burned,
            // and wake the main thread from its sleep.
            let _ = ctrlc::set_handler(move || {
                stop.store(true, Ordering::SeqCst);
                main.unpark();
            });
        }
        app::run(&ctx, command)
    });
    match result {
        Ok(reply) => {
            if json {
                println!("{}", ok_envelope(&reply.data));
            } else if !reply.human.is_empty() {
                println!("{}", reply.human);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json {
                println!("{}", err_envelope(&e));
            } else {
                eprintln!("falconer-hook: {}", e.message);
            }
            ExitCode::from(1)
        }
    }
}
