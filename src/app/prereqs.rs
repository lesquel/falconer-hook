//! What `setup` fixes on the host so the user does not have to: an agent
//! runtime (Herdr or tmux), a reachable SSH server and a running Tailscale.
//! Every command is shown first and runs only after a confirmation or
//! `--yes`; without a terminal it is printed, never run. Official installers
//! are downloaded over HTTPS to a private temp file and then run with `sh`,
//! never piped from the network.

use serde_json::{json, Map, Value};
use std::fmt::Write as _;

use super::{Ctx, Runtimes};
use crate::features::agents::domain::AgentError;
use crate::features::doctor::infrastructure::{check_ssh_listening, port_listening};
use crate::features::pairing::domain::{Tailnet, TailnetError};
use crate::features::pairing::infrastructure::tailscale::TailscaleCli;
use crate::features::setup::domain::prereqs::{
    consent, detect_downloader, detect_package_manager, detect_system_package_manager, display,
    enable_ssh_argv, install_source, installer_display, privileged, run_installer_argv,
    ssh_fix_hint, tailscale_up_argv, Consent, Downloader, Os, PackageManager, Source, HERDR,
    HERDR_INSTALLER_URL, HERDR_URL, HOMEBREW_URL, INSTALLER_FILE, TAILSCALE,
    TAILSCALE_DOWNLOAD_URL, TAILSCALE_INSTALLER_URL, TMUX,
};
use crate::features::setup::infrastructure::PrivateTempFile;
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

fn downloader(ctx: &Ctx) -> Option<Downloader> {
    detect_downloader(|b| on_path(b, ctx.path_env.as_deref()))
}

/// Where `sshd` lives when it is not on `PATH`. `FALCONER_SBIN_DIRS`
/// (colon-separated) replaces the list so tests never see the real one.
fn sbin_dirs() -> Vec<std::path::PathBuf> {
    match std::env::var_os("FALCONER_SBIN_DIRS") {
        Some(v) => std::env::split_paths(&v).collect(),
        None => ["/usr/sbin", "/usr/local/sbin", "/sbin", "/usr/bin"]
            .iter()
            .map(std::path::PathBuf::from)
            .collect(),
    }
}

/// Whether the OpenSSH server binary is on this machine. Always true on
/// macOS, which ships it.
pub fn sshd_installed(ctx: &Ctx) -> bool {
    Os::current() == Os::MacOs
        || on_path("sshd", ctx.path_env.as_deref())
        || sbin_dirs().iter().any(|d| d.join("sshd").is_file())
}

/// The package manager and package that would install the missing OpenSSH
/// server, `None` when it is installed or cannot be installed here.
pub fn ssh_server_install(ctx: &Ctx) -> Option<(PackageManager, &'static str)> {
    if sshd_installed(ctx) {
        return None;
    }
    let pm = detect_system_package_manager(|b| on_path(b, ctx.path_env.as_deref()))?;
    pm.ssh_server_package().map(|pkg| (pm, pkg))
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

/// Downloads an official installer to a private temp file, runs it with
/// `sh`, and deletes it.
fn run_installer(d: Downloader, url: &str) -> Result<(), String> {
    let tmp = PrivateTempFile::new(INSTALLER_FILE)
        .map_err(|e| format!("cannot create a private temp file: {e}"))?;
    let file = tmp.path().display().to_string();
    run(&d.argv(url, &file))?;
    if !tmp.has_content() {
        return Err(format!("downloading {url} produced no file"));
    }
    run(&run_installer_argv(&file))
}

/// `brew install herdr` when Homebrew is here, for `doctor`.
pub fn herdr_install_hint(ctx: &Ctx) -> Option<String> {
    package_manager(ctx)
        .filter(|pm| pm.has_herdr())
        .map(|pm| display(&pm.install_argv(HERDR)))
}

/// The JSON for one offered step: `action`, then `fields`, then `error`.
fn step(action: &str, fields: &Value, error: Option<String>) -> Value {
    let mut m = Map::new();
    m.insert("action".into(), json!(action));
    if let Value::Object(f) = fields {
        m.extend(f.clone());
    }
    if let Some(e) = error {
        m.insert("error".into(), json!(e));
    }
    Value::Object(m)
}

/// One thing `setup` can install.
struct Item<'a> {
    /// As shown to the user.
    name: &'a str,
    /// The package-manager package.
    package: &'a str,
    source: Source,
    /// The official installer, used with `Source::Installer`.
    installer_url: &'a str,
    /// Why an optional install is worth it.
    reason: Option<&'a str>,
    /// Go into the JSON before `command`.
    tags: Value,
}

/// Offers to install `item`, through the usual consent rules.
fn offer(ctx: &Ctx, item: Item, mode: &Mode, human: &mut String) -> Value {
    let Item {
        name,
        package,
        source,
        installer_url,
        reason,
        tags: mut fields,
    } = item;
    let why = reason.map(|r| format!(" ({r})")).unwrap_or_default();
    let argv = match source {
        Source::Package(pm) => privileged(pm.install_argv(package), pm.needs_root(), is_root(ctx)),
        Source::Installer(_) => Vec::new(),
    };
    let (cmd, question) = match source {
        Source::Package(_) => {
            let cmd = display(&argv);
            let q = format!("Install {name} with {cmd}{why}?");
            (cmd, q)
        }
        Source::Installer(d) => {
            let cmd = installer_display(d, installer_url);
            fields["url"] = json!(installer_url);
            let q = format!(
                "Install {name} with its official installer from {installer_url} \
                 (downloaded to a private temp file, then run): {cmd}{why}?"
            );
            (cmd, q)
        }
    };
    fields["command"] = json!(cmd);
    match consent(mode.yes, mode.interactive, || (mode.ask)(&question)) {
        Consent::PrintOnly => step("printed", &fields, None),
        Consent::Declined => step("declined", &fields, None),
        Consent::Run => {
            let ran = match source {
                Source::Package(_) => run(&argv),
                Source::Installer(d) => run_installer(d, installer_url),
            };
            match ran {
                Ok(()) => {
                    let _ = writeln!(human, "Installed {name} ({cmd})");
                    step("installed", &fields, None)
                }
                Err(e) => {
                    let _ = writeln!(human, "Could not install {name}: {e}");
                    step("failed", &fields, Some(e))
                }
            }
        }
    }
}

fn installed(action: &Value) -> bool {
    action["action"] == "installed"
}

/// Offers Herdr from `source`, tagged as a runtime.
fn offer_herdr(ctx: &Ctx, source: Source, mode: &Mode, human: &mut String) -> Value {
    let item = Item {
        name: "Herdr",
        package: HERDR,
        source,
        installer_url: HERDR_INSTALLER_URL,
        reason: Some("unlocks View as agents in the app"),
        tags: json!({"runtime": HERDR}),
    };
    offer(ctx, item, mode, human)
}

/// Makes sure an agent runtime exists. Herdr comes first (it unlocks View as
/// agents in the app): through Homebrew when it is here, else through its
/// official installer when curl or wget is. tmux from the package manager
/// is the fallback.
pub fn ensure_runtime(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let rts = Runtimes::new(ctx, cfg);
    let present = |r: &dyn crate::features::agents::domain::AgentRuntime| {
        !matches!(r.list_sessions(), Err(AgentError::Unavailable(_)))
    };
    let herdr_here = cfg.source_enabled("herdr") && present(&rts.herdr);
    let tmux_here = cfg.source_enabled("tmux") && present(&rts.tmux);
    let pm = package_manager(ctx);
    let herdr_source = install_source(pm, |pm| pm.has_herdr(), downloader(ctx));
    if herdr_here {
        return json!({"action": "already_installed", "runtime": "herdr"});
    }
    if tmux_here {
        // Optional upgrade: only when a person can answer.
        let Some(source) = herdr_source.filter(|_| !mode.yes && mode.interactive) else {
            return json!({"action": "already_installed", "runtime": "tmux"});
        };
        let r = offer_herdr(ctx, source, mode, human);
        return json!({"action": "already_installed", "runtime": "tmux", "herdr": r});
    }

    let herdr_url = format!("Herdr ({HERDR_URL}) is the other option.");
    let mut herdr_offer = None;
    if let Some(source) = herdr_source {
        let r = offer_herdr(ctx, source, mode, human);
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
        let mut r = json!({"action": "unavailable", "note": hint, "alternative": HERDR_URL});
        if let Some(h) = herdr_offer {
            r["herdr"] = h;
        }
        return r;
    };
    let item = Item {
        name: TMUX,
        package: TMUX,
        source: Source::Package(pm),
        installer_url: "",
        reason: None,
        tags: json!({"runtime": TMUX}),
    };
    let mut r = offer(ctx, item, mode, human);
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

/// Offers the OpenSSH server package when nothing listens and it is missing.
/// `None` when it is installed or cannot be installed here.
fn ensure_ssh_server(
    ctx: &Ctx,
    port: u16,
    enable: &str,
    mode: &Mode,
    human: &mut String,
) -> Option<Value> {
    let (pm, pkg) = ssh_server_install(ctx)?;
    let item = Item {
        name: "the OpenSSH server",
        package: pkg,
        source: Source::Package(pm),
        installer_url: "",
        reason: None,
        tags: json!({"package": pkg}),
    };
    let r = offer(ctx, item, mode, human);
    let cmd = r["command"].as_str().unwrap_or_default().to_string();
    match r["action"].as_str() {
        Some("printed") => {
            let _ = writeln!(
                human,
                "Nothing listens on SSH port {port} and the OpenSSH server is not installed.\n\
                 Install it with: {cmd}\nthen turn it on with: {enable}"
            );
        }
        Some("declined") => {
            let _ = writeln!(
                human,
                "OpenSSH server not installed (declined). Run `{cmd}` and `{enable}` when you are ready."
            );
        }
        _ => {}
    }
    Some(r)
}

/// Turns the SSH server on when nothing listens on the configured port.
pub fn ensure_ssh(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let port = cfg.ssh_port;
    if port_listening(port) {
        return json!({"action": "already_listening", "port": port});
    }
    let argv = enable_ssh_argv(Os::current(), package_manager(ctx), is_root(ctx));
    let cmd = display(&argv);
    let install = match ensure_ssh_server(ctx, port, &cmd, mode, human) {
        Some(r) if !installed(&r) => {
            let mut out = json!({
                "action": r["action"],
                "port": port,
                "command": r["command"],
                "package": r["package"],
                "enable": cmd,
            });
            if let Some(e) = r.get("error") {
                out["error"] = e.clone();
            }
            return out;
        }
        r => r,
    };
    let mut result = enable_ssh(ctx, port, &argv, &cmd, mode, human);
    if let (Some(i), Value::Object(m)) = (install, &mut result) {
        m.insert("install".into(), i);
    }
    result
}

fn enable_ssh(
    ctx: &Ctx,
    port: u16,
    argv: &[String],
    cmd: &str,
    mode: &Mode,
    human: &mut String,
) -> Value {
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
            let result = run(argv).and_then(|()| {
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

/// Makes sure Tailscale is installed and up: installs it when missing
/// (Homebrew, else its official installer), then offers `tailscale up` when
/// it is stopped or logged out.
pub fn ensure_tailscale(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let ts = TailscaleCli {
        runner: &ctx.runner,
        program: cfg.bins.tailscale.clone(),
    };
    match ts.status() {
        Ok(s) if s.running => json!({"action": "already_running"}),
        Err(TailnetError::Missing) => {
            let mut r = install_tailscale(ctx, mode, human);
            // A fresh install is logged out: offer to bring it up right away.
            let up_next = installed(&r) && !matches!(ts.status(), Ok(s) if s.running);
            if up_next && !matches!(ts.status(), Err(TailnetError::Missing)) {
                r["up"] = tailscale_up(ctx, cfg, mode, human);
            }
            r
        }
        _ => tailscale_up(ctx, cfg, mode, human),
    }
}

fn install_tailscale(ctx: &Ctx, mode: &Mode, human: &mut String) -> Value {
    let source = install_source(
        package_manager(ctx),
        |pm| pm.has_tailscale(),
        downloader(ctx),
    );
    let Some(source) = source else {
        let note = format!(
            "Install Tailscale from {TAILSCALE_DOWNLOAD_URL}, then run `sudo tailscale up`."
        );
        let _ = writeln!(human, "Tailscale is not installed. {note}");
        return json!({"action": "unavailable", "note": note});
    };
    let item = Item {
        name: "Tailscale",
        package: TAILSCALE,
        source,
        installer_url: TAILSCALE_INSTALLER_URL,
        reason: Some("required to pair a phone"),
        tags: json!({}),
    };
    let r = offer(ctx, item, mode, human);
    let cmd = r["command"].as_str().unwrap_or_default();
    match r["action"].as_str() {
        Some("printed") => {
            let _ = writeln!(human, "Tailscale is not installed. Install it with: {cmd}");
        }
        Some("declined") => {
            let _ = writeln!(
                human,
                "Tailscale not installed (declined). Run `{cmd}` when you are ready."
            );
        }
        _ => {}
    }
    r
}

/// `tailscale up` prints a login URL and waits for it, so it runs only on a
/// terminal a person watches, even with `--yes`.
fn tailscale_up(ctx: &Ctx, cfg: &Config, mode: &Mode, human: &mut String) -> Value {
    let argv = tailscale_up_argv(&cfg.bins.tailscale, Os::current(), is_root(ctx));
    let cmd = display(&argv);
    let fields = json!({"command": cmd});
    let decision = if mode.interactive {
        consent(mode.yes, true, || {
            (mode.ask)(&format!(
                "Tailscale is not running or logged out. Bring it up with {cmd} (it prints a login URL)?"
            ))
        })
    } else {
        Consent::PrintOnly
    };
    match decision {
        Consent::PrintOnly => {
            let _ = writeln!(
                human,
                "Tailscale is not running or logged out. Run `{cmd}` and open the login URL it prints."
            );
            step("printed", &fields, None)
        }
        Consent::Declined => {
            let _ = writeln!(
                human,
                "Tailscale left down (declined). Run `{cmd}` when you are ready."
            );
            step("declined", &fields, None)
        }
        Consent::Run => match run(&argv) {
            Ok(()) => {
                let _ = writeln!(human, "Tailscale is up ({cmd})");
                step("up", &fields, None)
            }
            Err(e) => {
                let _ = writeln!(human, "Could not bring Tailscale up: {e}");
                step("failed", &fields, Some(e))
            }
        },
    }
}

/// The doctor check for the SSH port, with the platform fix.
pub fn ssh_listening_check(ctx: &Ctx, port: u16) -> crate::features::doctor::domain::Check {
    check_ssh_listening(port, port_listening(port), &ssh_fix(ctx))
}
