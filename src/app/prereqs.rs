//! What `setup` fixes on the host so the user does not have to: an agent
//! runtime (tmux) and a reachable SSH server. Every command is shown first
//! and runs only after a confirmation or `--yes`; without a terminal it is
//! printed, never run.

use serde_json::{json, Value};
use std::fmt::Write as _;

use super::{Ctx, Runtimes};
use crate::features::agents::domain::AgentError;
use crate::features::doctor::infrastructure::{check_ssh_listening, port_listening};
use crate::features::setup::domain::prereqs::{
    consent, detect_package_manager, display, enable_ssh_argv, privileged, ssh_fix_hint, Consent,
    Os, PackageManager, HERDR_URL, HOMEBREW_URL, TMUX,
};
use crate::shared::config::Config;
use crate::shared::infrastructure::process::run_inherited;
use crate::shared::infrastructure::system::on_path;

/// How `setup` talks to the user.
pub struct Mode<'a> {
    pub yes: bool,
    pub interactive: bool,
    /// Asks `<question> [y/N]` and reads the answer.
    pub ask: &'a dyn Fn(&str) -> bool,
}

fn is_root(ctx: &Ctx) -> bool {
    ctx.user.as_deref() == Some("root")
}

pub fn package_manager(ctx: &Ctx) -> Option<PackageManager> {
    detect_package_manager(|b| on_path(b, ctx.path_env.as_deref()))
}

/// The fix shown by `doctor` when nothing listens on the SSH port.
pub fn ssh_fix(ctx: &Ctx) -> String {
    ssh_fix_hint(Os::current(), package_manager(ctx), is_root(ctx))
}

/// Runs a confirmed command on the user's terminal.
fn run(argv: &[String]) -> Result<(), String> {
    eprintln!("Running: {}", display(argv));
    match run_inherited(argv) {
        Ok(0) => Ok(()),
        Ok(code) => Err(format!("`{}` exited with {code}", display(argv))),
        Err(e) => Err(e.to_string()),
    }
}

/// Installs tmux when neither Herdr nor tmux is available.
pub fn ensure_runtime(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let rts = Runtimes::new(ctx, cfg);
    let found = rts
        .enabled()
        .iter()
        .any(|r| !matches!(r.list_sessions(), Err(AgentError::Unavailable(_))));
    if found {
        return json!({"action": "already_installed"});
    }
    let herdr = format!("Herdr ({HERDR_URL}) is the other option; install it from its site.");
    let Some(pm) = package_manager(ctx) else {
        let hint = if cfg!(target_os = "macos") {
            format!("Install Homebrew from {HOMEBREW_URL}, then run `brew install tmux`.")
        } else {
            "Install tmux with your system's package manager.".to_string()
        };
        let _ = writeln!(
            human,
            "No agent runtime (herdr or tmux) found and no supported package manager. {hint}\n{herdr}"
        );
        return json!({"action": "unavailable", "note": hint, "alternative": HERDR_URL});
    };
    let argv = privileged(pm.install_argv(TMUX), pm.needs_root(), is_root(ctx));
    let cmd = display(&argv);
    let decision = consent(mode.yes, mode.interactive, || {
        (mode.ask)(&format!("Install tmux with {cmd}?"))
    });
    match decision {
        Consent::PrintOnly => {
            let _ = writeln!(
                human,
                "No agent runtime (herdr or tmux) found. Install tmux with: {cmd}\n{herdr}"
            );
            json!({"action": "printed", "command": cmd, "alternative": HERDR_URL})
        }
        Consent::Declined => {
            let _ = writeln!(
                human,
                "tmux not installed (declined). Run `{cmd}` when you are ready.\n{herdr}"
            );
            json!({"action": "declined", "command": cmd, "alternative": HERDR_URL})
        }
        Consent::Run => match run(&argv) {
            Ok(()) => {
                let _ = writeln!(human, "Installed tmux ({cmd})");
                json!({"action": "installed", "command": cmd})
            }
            Err(e) => {
                let _ = writeln!(human, "Could not install tmux: {e}\n{herdr}");
                json!({"action": "failed", "command": cmd, "error": e})
            }
        },
    }
}

/// Turns the SSH server on when nothing listens on the configured port.
pub fn ensure_ssh(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let port = cfg.ssh_port;
    if port_listening(port) {
        return json!({"action": "already_listening", "port": port});
    }
    let argv = enable_ssh_argv(Os::current(), package_manager(ctx), is_root(ctx));
    let cmd = display(&argv);
    let decision = consent(mode.yes, mode.interactive, || {
        (mode.ask)(&format!(
            "Nothing listens on SSH port {port}. Turn it on with {cmd}?"
        ))
    });
    match decision {
        Consent::PrintOnly => {
            let _ = writeln!(
                human,
                "Nothing listens on SSH port {port}. Fix: {}",
                ssh_fix(ctx)
            );
            json!({"action": "printed", "port": port, "command": cmd})
        }
        Consent::Declined => {
            let _ = writeln!(
                human,
                "SSH server left off (declined). Run `{cmd}` when you are ready."
            );
            json!({"action": "declined", "port": port, "command": cmd})
        }
        Consent::Run => {
            let result = run(&argv).and_then(|()| {
                // The server needs a moment to bind.
                for _ in 0..10 {
                    if port_listening(port) {
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                Err(format!("ran, but nothing listens on port {port} yet"))
            });
            match result {
                Ok(()) => {
                    let _ = writeln!(human, "SSH server is on ({cmd})");
                    json!({"action": "enabled", "port": port, "command": cmd})
                }
                Err(e) => {
                    let _ = writeln!(human, "Could not turn on SSH: {e}. Fix: {}", ssh_fix(ctx));
                    json!({"action": "failed", "port": port, "command": cmd, "error": e})
                }
            }
        }
    }
}

/// The doctor check for the SSH port, with the platform fix.
pub fn ssh_listening_check(ctx: &Ctx, port: u16) -> crate::features::doctor::domain::Check {
    check_ssh_listening(port, port_listening(port), &ssh_fix(ctx))
}
