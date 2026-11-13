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
    Os, PackageManager, HERDR, HERDR_URL, HOMEBREW_URL, TMUX,
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

/// `brew install herdr` when Homebrew is here, for `doctor`.
pub fn herdr_install_hint(ctx: &Ctx) -> Option<String> {
    package_manager(ctx)
        .filter(|pm| pm.has_herdr())
        .map(|pm| display(&pm.install_argv(HERDR)))
}

/// Offers one package. `reason` explains an optional install.
fn offer(
    ctx: &Ctx,
    pm: PackageManager,
    package: &str,
    reason: Option<&str>,
    mode: &Mode,
    human: &mut String,
) -> Value {
    let argv = privileged(pm.install_argv(package), pm.needs_root(), is_root(ctx));
    let cmd = display(&argv);
    let name = if package == HERDR { "Herdr" } else { package };
    let decision = consent(mode.yes, mode.interactive, || {
        let why = reason.map(|r| format!(" ({r})")).unwrap_or_default();
        (mode.ask)(&format!("Install {name} with {cmd}{why}?"))
    });
    match decision {
        Consent::PrintOnly => json!({"action": "printed", "runtime": package, "command": cmd}),
        Consent::Declined => json!({"action": "declined", "runtime": package, "command": cmd}),
        Consent::Run => match run(&argv) {
            Ok(()) => {
                let _ = writeln!(human, "Installed {name} ({cmd})");
                json!({"action": "installed", "runtime": package, "command": cmd})
            }
            Err(e) => {
                let _ = writeln!(human, "Could not install {name}: {e}");
                json!({"action": "failed", "runtime": package, "command": cmd, "error": e})
            }
        },
    }
}

fn installed(action: &Value) -> bool {
    action["action"] == "installed"
}

/// Makes sure an agent runtime exists. With Homebrew, Herdr comes first (it
/// unlocks View as agents in the app) and tmux is the fallback; elsewhere
/// tmux is offered and Herdr is only mentioned.
pub fn ensure_runtime(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let rts = Runtimes::new(ctx, cfg);
    let present = |r: &dyn crate::features::agents::domain::AgentRuntime| {
        !matches!(r.list_sessions(), Err(AgentError::Unavailable(_)))
    };
    let herdr_here = cfg.source_enabled("herdr") && present(&rts.herdr);
    let tmux_here = cfg.source_enabled("tmux") && present(&rts.tmux);
    let pm = package_manager(ctx);
    let brew = pm.filter(|pm| pm.has_herdr());
    if herdr_here {
        return json!({"action": "already_installed", "runtime": "herdr"});
    }
    if tmux_here {
        // Optional upgrade: only when a person can answer.
        let Some(brew) = brew.filter(|_| !mode.yes && mode.interactive) else {
            return json!({"action": "already_installed", "runtime": "tmux"});
        };
        let r = offer(
            ctx,
            brew,
            HERDR,
            Some("unlocks View as agents in the app"),
            mode,
            human,
        );
        return json!({"action": "already_installed", "runtime": "tmux", "herdr": r});
    }

    let herdr_url = format!("Herdr ({HERDR_URL}) is the other option.");
    let mut herdr_offer = None;
    if let Some(brew) = brew {
        let r = offer(
            ctx,
            brew,
            HERDR,
            Some("unlocks View as agents in the app"),
            mode,
            human,
        );
        if installed(&r) {
            return r;
        }
        herdr_offer = Some(r);
    }
    let Some(pm) = pm else {
        let hint = if cfg!(target_os = "macos") {
            format!("Install Homebrew from {HOMEBREW_URL}, then run `brew install herdr` or `brew install tmux`.")
        } else {
            "Install tmux with your system's package manager.".to_string()
        };
        let _ = writeln!(
            human,
            "No agent runtime (herdr or tmux) found and no supported package manager. {hint}\n{herdr_url}"
        );
        return json!({"action": "unavailable", "note": hint, "alternative": HERDR_URL});
    };
    let mut r = offer(ctx, pm, TMUX, None, mode, human);
    let cmd = r["command"].as_str().unwrap_or_default().to_string();
    match r["action"].as_str() {
        Some("printed") => {
            let herdr_cmd = herdr_offer
                .as_ref()
                .and_then(|h| h["command"].as_str())
                .map(|c| format!("Install Herdr with: {c}\nor tmux with: {cmd}"))
                .unwrap_or_else(|| format!("Install tmux with: {cmd}\n{herdr_url}"));
            let _ = writeln!(
                human,
                "No agent runtime (herdr or tmux) found.\n{herdr_cmd}"
            );
        }
        Some("declined") => {
            let _ = writeln!(
                human,
                "No agent runtime installed (declined). Run `{cmd}` when you are ready."
            );
        }
        _ => {}
    }
    if let (Some(h), Value::Object(m)) = (herdr_offer, &mut r) {
        m.insert("herdr".into(), h);
    } else if let Value::Object(m) = &mut r {
        m.insert("alternative".into(), json!(HERDR_URL));
    }
    r
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
