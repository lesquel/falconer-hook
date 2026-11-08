//! `openmosh-hook` entry point. With `app/`, this is the composition root:
//! the only place that knows concrete adapter types.
#![forbid(unsafe_code)]

mod app;
mod cli;
mod features;
mod shared;
#[cfg(test)]
mod test_vectors;

use clap::Parser;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

use cli::output::{err_envelope, ok_envelope};
use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.command.json();
    let result = app::Ctx::from_env().and_then(|ctx| {
        if matches!(cli.command, Command::Serve | Command::Pair { .. }) {
            let stop = ctx.stop.clone();
            // SIGINT/SIGTERM: stop the loop so the pairing token is burned.
            let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));
        }
        app::run(&ctx, cli.command)
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
                eprintln!("openmosh-hook: {}", e.message);
            }
            ExitCode::from(1)
        }
    }
}
