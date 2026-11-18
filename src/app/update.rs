//! `update`: compares the running version with the latest release and
//! reinstalls through that release's own install.sh, downloaded (pinned to
//! the tag) into a private temp file and run only after consent. The script
//! verifies the signed SHA256SUMS itself; nothing is verified twice here.

use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::prereqs::Mode;
use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::features::service::domain::SYSTEMD_UNIT_NAME;
use crate::features::setup::domain::prereqs::{consent, Consent};
use crate::features::update::domain::{
    installer_args, script_url, valid_tag, UpdateCheck, UpdateError, Version,
};
use crate::features::update::infrastructure::{
    download, resolve_latest, run_installer, Downloader, PrivateTemp,
};
use crate::shared::infrastructure::process::ProcessRunner;

/// The running version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Network calls get longer than the default 10 s.
fn net_runner() -> ProcessRunner {
    ProcessRunner {
        timeout: Duration::from_secs(60),
    }
}

fn downloader(ctx: &Ctx) -> Result<Downloader, UpdateError> {
    Downloader::detect(ctx.path_env.as_deref()).ok_or(UpdateError::NoDownloader)
}

/// Resolves the latest release and compares it with the running version.
/// Read-only; this is what `update --check` reports.
pub fn check(ctx: &Ctx) -> Result<UpdateCheck, CliError> {
    let tag = resolve_latest(&net_runner(), downloader(ctx)?)?;
    Ok(UpdateCheck::new(CURRENT, &tag)?)
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes" | "YES")
}

fn interactive(json: bool) -> bool {
    // `FALCONER_ASSUME_TTY=1` lets tests drive the prompt through a pipe.
    !json && (std::io::stdin().is_terminal() || std::env::var_os("FALCONER_ASSUME_TTY").is_some())
}

/// The installed service's file, when there is one.
fn service_file(ctx: &Ctx) -> PathBuf {
    if cfg!(target_os = "macos") {
        ctx.paths.launchd_plist()
    } else {
        ctx.paths.systemd_unit()
    }
}

/// Quotes a word for a shell only when it needs it.
fn sh_word(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-=+:,@%".contains(&b))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The two commands a person would type to do the same by hand.
fn manual_commands(d: Downloader, url: &str, args: &[String], bin_dir: &Path) -> [String; 2] {
    let dl = d
        .download_argv(url, Path::new("install.sh"))
        .iter()
        .map(|a| sh_word(a))
        .collect::<Vec<_>>()
        .join(" ");
    let run = format!(
        "FALCONER_SKIP_PREREQS=1 FALCONER_BIN_DIR={} sh install.sh {}",
        sh_word(&bin_dir.display().to_string()),
        args.join(" ")
    );
    [dl, run]
}

/// Restarts the service so it runs the new binary (`service install`, which
/// install.sh runs, only starts a stopped one).
fn restart_service(ctx: &Ctx, unit: &Path) -> Value {
    let cfg = ctx.config().unwrap_or_default();
    let cmds: Vec<Vec<String>> = if cfg!(target_os = "macos") {
        let p = unit.display().to_string();
        vec![
            vec![cfg.bins.launchctl.clone(), "unload".into(), p.clone()],
            vec![cfg.bins.launchctl.clone(), "load".into(), "-w".into(), p],
        ]
    } else {
        vec![vec![
            cfg.bins.systemctl.clone(),
            "--user".into(),
            "restart".into(),
            SYSTEMD_UNIT_NAME.into(),
        ]]
    };
    for argv in &cmds {
        let res =
            crate::shared::domain::command::CommandRunner::run(&ctx.runner, &argv[0], &argv[1..]);
        let err = match res {
            Ok(o) if o.success() => continue,
            Ok(o) => format!("`{}` exited with {}", argv.join(" "), o.status),
            Err(e) => e.to_string(),
        };
        return json!({"restarted": false, "error": err});
    }
    json!({"restarted": true})
}

pub fn update(
    ctx: &Ctx,
    check_only: bool,
    version: Option<&str>,
    yes: bool,
    json: bool,
) -> Result<Reply, CliError> {
    let info = match version {
        Some(t) => {
            if !valid_tag(t) || Version::parse(t).is_none() {
                return Err(CliError::new(
                    "invalid_input",
                    "--version must be a release tag like v0.2.1",
                ));
            }
            UpdateCheck::new(CURRENT, t)?
        }
        None => check(ctx)?,
    };
    let base = json!({
        "current": info.current,
        "latest": info.latest,
        "tag": info.tag,
        "update_available": info.update_available,
    });
    let with = |extra: Value| {
        let mut v = base.clone();
        if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
            m.extend(e);
        }
        v
    };
    if check_only {
        return Ok(Reply::new(&base, info.summary()));
    }
    if version.is_none() && !info.update_available {
        return Ok(Reply::new(
            &with(json!({"action": "up_to_date"})),
            info.summary(),
        ));
    }

    let headline = if version.is_some() {
        format!("Install {} (running v{})", info.tag, info.current)
    } else {
        info.summary()
    };
    let d = downloader(ctx)?;
    let url = script_url(&info.tag);
    let unit = service_file(ctx);
    let has_service = unit.exists();
    let args = installer_args(&info.tag, has_service);
    let bin_dir = ctx
        .exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ctx.paths.home.join(".local/bin"));
    let [dl_cmd, run_cmd] = manual_commands(d, &url, &args, &bin_dir);
    let command = format!("{dl_cmd} && {run_cmd}");
    let mode = Mode {
        yes,
        interactive: interactive(json),
        ask: &ask,
    };
    let decision = consent(mode.yes, mode.interactive, || {
        eprintln!("{headline}\nDownload: {url}\nRun: {run_cmd}");
        eprintln!("install.sh verifies the release's signed SHA256SUMS before installing.");
        (mode.ask)(&format!("Update falconer-hook to {}?", info.tag))
    });
    let mut human = String::new();
    let _ = writeln!(human, "{headline}");
    match decision {
        Consent::PrintOnly => {
            let _ = write!(
                human,
                "To update, run:\n  {dl_cmd}\n  {run_cmd}\nor run `falconer-hook update --yes`."
            );
            Ok(Reply::new(
                &with(json!({"action": "printed", "script_url": url, "command": command})),
                human,
            ))
        }
        Consent::Declined => {
            let _ = write!(human, "Update declined; nothing changed.");
            Ok(Reply::new(
                &with(json!({"action": "declined", "script_url": url, "command": command})),
                human,
            ))
        }
        Consent::Run => {
            let tmp = PrivateTemp::new()?;
            let script = tmp.file("install.sh")?;
            download(&net_runner(), d, &url, &script)?;
            eprintln!(
                "Running: FALCONER_SKIP_PREREQS=1 FALCONER_BIN_DIR={} sh {} {}",
                sh_word(&bin_dir.display().to_string()),
                script.display(),
                args.join(" ")
            );
            let code = run_installer(&script, &args, &bin_dir, json)?;
            drop(tmp);
            if code != 0 {
                return Err(UpdateError::Failed(format!(
                    "install.sh exited with {code}; see its output above"
                ))
                .into());
            }
            let _ = write!(human, "Updated falconer-hook to {}", info.tag);
            let service = if has_service {
                let s = restart_service(ctx, &unit);
                match s["error"].as_str() {
                    None => {
                        let _ = write!(human, "\nRestarted the service");
                    }
                    Some(e) => {
                        let _ = write!(human, "\nCould not restart the service: {e}");
                    }
                }
                s
            } else {
                json!({"skipped": true})
            };
            Ok(Reply::new(
                &with(json!({
                    "action": "updated",
                    "script_url": url,
                    "command": command,
                    "service": service,
                })),
                human,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_are_quoted_only_when_needed() {
        assert_eq!(sh_word("/home/me/.local/bin"), "/home/me/.local/bin");
        assert_eq!(sh_word("=https"), "=https");
        assert_eq!(sh_word("/a b/it's"), r"'/a b/it'\''s'");
    }

    #[test]
    fn manual_commands_mirror_what_runs() {
        let [dl, run] = manual_commands(
            Downloader::Curl,
            "https://raw.githubusercontent.com/lesquel/falconer-hook/v0.2.1/install.sh",
            &installer_args("v0.2.1", false),
            Path::new("/home/me/.local/bin"),
        );
        assert_eq!(dl, "curl -fsSL --proto =https --tlsv1.2 -o install.sh https://raw.githubusercontent.com/lesquel/falconer-hook/v0.2.1/install.sh");
        assert_eq!(
            run,
            "FALCONER_SKIP_PREREQS=1 FALCONER_BIN_DIR=/home/me/.local/bin sh install.sh --version v0.2.1 --no-service"
        );
    }
}
