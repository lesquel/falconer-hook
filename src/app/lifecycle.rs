//! `setup` and `uninstall`: compose config, service, integrations, pairing
//! keys and doctor into one command each.

use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, IsTerminal, Write as _};
use std::path::Path;

use super::{commands, integrations, pair, prereqs, Ctx};
use crate::cli::output::{CliError, Reply};
use crate::features::integration::domain::CATALOG;
use crate::features::pairing::domain::{AuthorizedKeys, DeviceRegistry};
use crate::features::pairing::infrastructure::authorized_keys::FileAuthorizedKeys;
use crate::features::service::application::ApplyPlan;
use crate::features::service::domain::{uninstall_plan, Platform};
use crate::features::service::infrastructure::FsServiceFiles;
use crate::features::setup::application::{execute, UninstallPorts};
use crate::features::setup::domain::{deletable_binary, deletable_dir, plan_uninstall, Facts};
use crate::shared::config::{Config, DEFAULT_CONFIG};
use crate::shared::infrastructure::fs_secure::{ensure_private_dir, write_atomic};

/// Asks a yes/no question on the terminal (stderr prompt, stdin answer).
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
    // `FALCONER_ASSUME_TTY=1` lets tests drive the prompts through a pipe.
    !json && (std::io::stdin().is_terminal() || std::env::var_os("FALCONER_ASSUME_TTY").is_some())
}

/// Asks `<question> [Y/n]`; Enter means yes.
fn ask_default_yes(question: &str) -> bool {
    eprint!("{question} [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    !matches!(line.trim(), "n" | "N" | "no" | "NO")
}

fn platform_unit(ctx: &Ctx) -> (Platform, std::path::PathBuf) {
    if cfg!(target_os = "macos") {
        (Platform::Launchd, ctx.paths.launchd_plist())
    } else {
        (Platform::Systemd, ctx.paths.systemd_unit())
    }
}

fn write_default_config(ctx: &Ctx) -> Result<bool, CliError> {
    let file = ctx.paths.config_file();
    if file.exists() {
        return Ok(false);
    }
    ensure_private_dir(&ctx.paths.config_dir)?;
    write_atomic(&file, DEFAULT_CONFIG.as_bytes(), 0o600)?;
    Ok(true)
}

pub fn setup(ctx: &Ctx, yes: bool, no_service: bool, json: bool) -> Result<Reply, CliError> {
    let mut human = String::new();
    let config_file = ctx.paths.config_file();
    let created = write_default_config(ctx)?;
    let _ = writeln!(
        human,
        "{} {}",
        if created { "Wrote" } else { "Kept" },
        config_file.display()
    );

    let service = if no_service {
        json!({"skipped": true})
    } else {
        match commands::service(ctx, true, false) {
            Ok(r) => {
                let _ = writeln!(human, "Installed the user service");
                json!({"installed": true, "plan": r.data})
            }
            Err(e) => {
                let _ = writeln!(human, "Service not installed: {}", e.message);
                json!({"installed": false, "error": e.message})
            }
        }
    };

    let ask_user = !yes && interactive(json);
    let mut agents = Vec::new();
    for st in integrations::statuses(ctx)
        .into_iter()
        .filter(|s| s.detected)
    {
        let action = match (st.installed, &st.error) {
            (Some(true), _) => json!({"action": "already_installed"}),
            (None, Some(e)) => json!({"action": "error", "error": e}),
            _ if yes
                || (ask_user
                    && ask(&format!(
                        "Install the {} integration ({})?",
                        st.name, st.settings
                    ))) =>
            {
                let id = CATALOG
                    .iter()
                    .find(|s| s.id.as_str() == st.agent)
                    .map(|s| s.id)
                    .ok_or_else(|| CliError::new("internal", "unknown agent"))?;
                match integrations::apply(ctx, id, None, true) {
                    Ok(r) => json!({"action": "installed", "note": r.note}),
                    Err(e) => json!({"action": "failed", "error": e.message}),
                }
            }
            _ => json!({"action": "skipped"}),
        };
        let _ = writeln!(
            human,
            "{:<19} {}{}",
            st.name,
            action["action"]
                .as_str()
                .unwrap_or_default()
                .replace('_', " "),
            action["error"]
                .as_str()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        );
        let mut entry = json!({"agent": st.agent, "settings": st.settings});
        if let (Value::Object(m), Value::Object(a)) = (&mut entry, action) {
            m.extend(a);
        }
        agents.push(entry);
    }
    if agents.is_empty() {
        let _ = writeln!(
            human,
            "No supported agent detected (see `falconer-hook integration list`)."
        );
    } else if !yes && !ask_user {
        let _ = writeln!(
            human,
            "Run `falconer-hook setup --yes` or `falconer-hook integration install <agent>` to add the skipped integrations."
        );
    }

    let cfg = ctx.config().unwrap_or_default();
    let mode = prereqs::Mode {
        yes,
        interactive: interactive(json),
        ask: &ask,
    };
    let runtime = prereqs::ensure_runtime(ctx, &cfg, &mode, &mut human);
    let ssh = prereqs::ensure_ssh(ctx, &cfg, &mode, &mut human);

    let doctor = commands::doctor(ctx)?;
    let _ = writeln!(human, "\n{}", doctor.human);
    let healthy = doctor.data["healthy"] == true;
    let unpaired = ctx.registry().list().map(|d| d.is_empty()).unwrap_or(false);
    let next_step = "Next: run `falconer-hook pair` and scan the QR code with Falconer.";
    let pairing = if !healthy || !unpaired {
        let _ = write!(human, "\n{next_step}");
        json!({"action": "skipped", "reason": if healthy { "already_paired" } else { "checks_failed" }})
    } else if yes || !mode.interactive {
        // Pairing needs a person to scan the QR code and confirm the phone.
        let _ = write!(human, "\n{next_step}");
        json!({"action": "skipped", "reason": "needs_a_person"})
    } else if ask_default_yes("Pair a phone now?") {
        // Show everything so far before the QR code takes the screen.
        println!("{}", human.trim_end());
        human.clear();
        match pair::pair(ctx, None, 300, false, false, false) {
            Ok(r) => {
                let _ = write!(human, "\n{}", r.human);
                json!({"action": "paired", "device": r.data})
            }
            Err(e) => {
                let _ = write!(
                    human,
                    "\nPairing did not finish: {}\n{next_step}",
                    e.message
                );
                json!({"action": "failed", "error": e.message})
            }
        }
    } else {
        let _ = write!(human, "\n{next_step}");
        json!({"action": "declined"})
    };
    let data = json!({
        "config": {"path": config_file, "created": created},
        "service": service,
        "integrations": agents,
        "prerequisites": {"agent_runtime": runtime, "ssh": ssh},
        "doctor": doctor.data,
        "pairing": pairing,
        "next": "falconer-hook pair",
    });
    Ok(Reply::new(&data, human))
}

fn keys(ctx: &Ctx) -> FileAuthorizedKeys {
    FileAuthorizedKeys {
        path: ctx.paths.authorized_keys(),
        lock_path: ctx.lock_path(),
    }
}

/// Read-only inventory of what falconer-hook created.
fn facts(ctx: &Ctx) -> Result<Facts, CliError> {
    let (_, unit) = platform_unit(ctx);
    let integrations = integrations::statuses(ctx)
        .into_iter()
        .filter(|s| s.installed == Some(true))
        .map(|s| (s.agent, s.settings))
        .collect();
    Ok(Facts {
        unit: unit.exists().then_some(unit),
        integrations,
        authorized_keys: ctx.paths.authorized_keys(),
        tagged_keys: keys(ctx).remove_tagged(true)?,
        dirs: [&ctx.paths.config_dir, &ctx.paths.state_dir]
            .into_iter()
            .filter(|d| d.exists())
            .cloned()
            .collect(),
        binary: ctx.exe.clone(),
    })
}

struct Ports<'a> {
    ctx: &'a Ctx,
    cfg: Config,
}

impl UninstallPorts for Ports<'_> {
    fn service(&self, _unit: &Path) -> Result<(), String> {
        let (platform, unit) = platform_unit(self.ctx);
        let plan = uninstall_plan(
            platform,
            &unit,
            &self.cfg.bins.systemctl,
            &self.cfg.bins.launchctl,
        )
        .map_err(|e| e.to_string())?;
        ApplyPlan {
            files: &FsServiceFiles,
            runner: &self.ctx.runner,
        }
        .apply(&plan)
        .map_err(|e| e.to_string())
    }

    fn integration(&self, agent: &str) -> Result<(), String> {
        let id = CATALOG
            .iter()
            .find(|s| s.id.as_str() == agent)
            .map(|s| s.id)
            .ok_or("unknown agent")?;
        integrations::apply(self.ctx, id, None, false)
            .map(|_| ())
            .map_err(|e| e.message)
    }

    fn authorized_keys(&self) -> Result<usize, String> {
        // The edit lock lives in the state dir; recreate it briefly if the
        // user already deleted it.
        let state = &self.ctx.paths.state_dir;
        let created = !state.exists();
        if created {
            ensure_private_dir(state).map_err(|e| e.to_string())?;
        }
        let r = keys(self.ctx)
            .remove_tagged(false)
            .map_err(|e| e.to_string());
        if created && deletable_dir(state) {
            let _ = std::fs::remove_dir_all(state);
        }
        r
    }

    fn remove_dir(&self, path: &Path) -> Result<(), String> {
        if !deletable_dir(path) {
            return Err("refusing to delete a directory not named falconer".into());
        }
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    }

    fn remove_binary(&self, path: &Path) -> Result<(), String> {
        if !deletable_binary(path) {
            return Err("refusing to delete a file not named falconer-hook".into());
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

/// What uninstall deliberately leaves, so the user can decide: the sshd
/// drop-in from `harden-ssh` needs root to remove and turning it off would
/// re-enable password logins, so it is listed, never removed.
pub fn kept_notes(drop_in_exists: bool) -> Vec<String> {
    let mut v = Vec::new();
    if drop_in_exists {
        v.push(format!(
            "{} (from `harden-ssh`: sshd accepts SSH keys only). Kept on purpose; to allow \
             password logins again: `sudo rm {}` and reload sshd",
            super::harden::DROP_IN,
            super::harden::DROP_IN
        ));
    }
    v
}

pub fn uninstall(
    ctx: &Ctx,
    all: bool,
    dry_run: bool,
    yes: bool,
    json: bool,
) -> Result<Reply, CliError> {
    // The service step needs the configured service manager binaries.
    let cfg = ctx.config().unwrap_or_default();
    let steps = plan_uninstall(facts(ctx)?, all);
    let kept = kept_notes(std::path::Path::new(super::harden::DROP_IN).exists());
    let mut kept_text = String::new();
    for k in &kept {
        let _ = write!(kept_text, "\n- keep {k}");
    }
    let mut plan_text = String::new();
    for s in &steps {
        let _ = writeln!(plan_text, "- {}", s.describe());
    }
    if steps.is_empty() {
        return Ok(Reply::new(
            &json!({"dry_run": dry_run, "steps": steps, "complete": true, "kept": kept}),
            format!("Nothing to remove: falconer-hook left nothing on this machine.{kept_text}"),
        ));
    }
    if dry_run {
        return Ok(Reply::new(
            &json!({"dry_run": true, "steps": steps, "kept": kept}),
            format!(
                "falconer-hook uninstall would:\n{}{kept_text}",
                plan_text.trim_end()
            ),
        ));
    }
    if !yes {
        if !interactive(json) {
            return Err(CliError::new(
                "confirmation_required",
                "uninstall needs confirmation: pass --yes, or --dry-run to see the plan",
            ));
        }
        eprintln!(
            "falconer-hook uninstall will:\n{}{kept_text}",
            plan_text.trim_end()
        );
        if !ask("Proceed?") {
            return Err(CliError::new(
                "cancelled",
                "uninstall cancelled; nothing changed",
            ));
        }
    }
    let outcomes = execute(&Ports { ctx, cfg }, steps);
    let complete = outcomes.iter().all(|o| o.ok);
    let mut human = String::new();
    for o in &outcomes {
        let _ = writeln!(
            human,
            "[{}] {}{}",
            if o.ok { "ok  " } else { "FAIL" },
            o.step.describe(),
            o.error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        );
    }
    human.push_str(if complete {
        "falconer-hook was removed."
    } else {
        "Some steps failed; fix them and run `falconer-hook uninstall` again."
    });
    human.push_str(&kept_text);
    Ok(Reply::new(
        &json!({"dry_run": false, "outcomes": outcomes, "complete": complete, "kept": kept}),
        human,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sshd_drop_in_is_listed_as_kept() {
        assert!(kept_notes(false).is_empty());
        let k = kept_notes(true);
        assert_eq!(k.len(), 1);
        assert!(k[0].starts_with("/etc/ssh/sshd_config.d/10-falconer.conf"));
        assert!(k[0].contains("sudo rm /etc/ssh/sshd_config.d/10-falconer.conf"));
    }
}
