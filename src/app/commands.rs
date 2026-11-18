//! Short request/response commands.

use serde_json::json;
use std::fmt::Write as _;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::{open_state, with_notifier, Ctx, Runtimes};
use crate::cli::output::{CliError, Reply};
use crate::cli::{ActionArg, KindArg};
use crate::features::agents::application::{ListAgents, ListSessions};
use crate::features::doctor::application::{
    check_devices, check_keymaps, check_runtimes, check_tailscale, check_tailscale_ssh,
};
use crate::features::doctor::domain::{Check, DoctorReport, FixId, Status};
use crate::features::doctor::infrastructure::{
    check_authorized_keys, check_launchd_service, check_permissions, check_ssh, check_sshd_auth,
    check_systemd_service, sshd_password_logins,
};
use crate::features::events::application::{IngestEvent, IngestOutcome, ListInbox};
use crate::features::events::domain::{parse_hook_payload, parse_with_kind, EmitKind, EventKind};
use crate::features::pairing::application::DeviceAdmin;
use crate::features::pairing::infrastructure::tailscale::TailscaleCli;
use crate::features::responses::application::RespondToEvent;
use crate::features::responses::domain::Action;
use crate::features::responses::infrastructure::EmbeddedKeymaps;
use crate::features::service::application::ApplyPlan;
use crate::features::service::domain::{
    install_plan, legacy_uninstall_plan, uninstall_plan, Platform,
};
use crate::features::service::infrastructure::FsServiceFiles;
use crate::features::setup::domain::prereqs::{consent, Consent};
use crate::shared::config::Config;
use crate::shared::domain::clock::Clock;
use crate::shared::infrastructure::system::{OsIds, SystemClock};

/// Reads stdin, refusing more than `max` bytes.
pub fn read_stdin(max: usize) -> Result<String, CliError> {
    let mut buf = Vec::new();
    std::io::stdin()
        .take(max as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| CliError::new("invalid_input", "could not read stdin"))?;
    if buf.len() > max {
        return Err(CliError::new("invalid_input", "stdin input is too large"));
    }
    String::from_utf8(buf).map_err(|_| CliError::new("invalid_input", "stdin is not UTF-8"))
}

/// The hook's pane, plus the tmux `pane_pid` it has now: `respond` later
/// refuses to type into that pane unless the same process still owns it.
/// Best effort: without tmux the pid stays unknown (and tmux answers are refused).
fn pane_with_pid(ctx: &Ctx, cfg: &Config) -> Option<crate::features::events::domain::PaneRef> {
    use crate::features::agents::domain::{AgentRuntime, PaneProbe};
    use crate::features::events::domain::Runtime;
    let mut pane = ctx.pane.clone()?;
    if pane.runtime == Runtime::Tmux {
        let rts = Runtimes::new(ctx, cfg);
        if let Ok(PaneProbe::Live(s)) = rts.tmux.probe(&pane) {
            pane.context.pane_pid = s.pane_pid;
        }
    }
    Some(pane)
}

pub fn emit(ctx: &Ctx, source: &str, kind: Option<KindArg>) -> Result<Reply, CliError> {
    use std::io::IsTerminal;
    let cfg = ctx.config()?;
    // With --kind the payload is optional, and a hook run from an
    // interactive shell (e.g. Aider) may inherit the terminal: never block on it.
    let raw = if kind.is_some() && std::io::stdin().is_terminal() {
        String::new()
    } else {
        read_stdin(1024 * 1024)?
    };
    let (pane, now) = (pane_with_pid(ctx, &cfg), SystemClock.now());
    let new = match kind {
        None => parse_hook_payload(source, &raw, pane, now)?,
        Some(k) => {
            let k = match k {
                KindArg::ApprovalRequired => EmitKind::Fixed(EventKind::ApprovalRequired),
                KindArg::Question => EmitKind::Fixed(EventKind::Question),
                KindArg::Done => EmitKind::Fixed(EventKind::Done),
                KindArg::Notification => EmitKind::Notification,
            };
            parse_with_kind(source, k, &raw, pane, now)?
        }
    };
    let st = open_state(ctx)?;
    let ingest = IngestEvent {
        store: &st.store,
        ids: &OsIds,
        clock: &SystemClock,
        retention_days: cfg.retention_days,
    };
    match ingest.ingest(new)? {
        IngestOutcome::Duplicate => Ok(Reply::new(
            &json!({"recorded": false, "duplicate": true}),
            "",
        )),
        IngestOutcome::Recorded { event, superseded } => with_notifier(ctx, &cfg, &st, |n| {
            for old in &superseded {
                n.notify_resolved(old);
            }
            let pushed = n.notify(&event);
            Ok(Reply::new(
                &json!({"recorded": true, "event_id": event.id, "seq": event.seq, "pushed": pushed}),
                "",
            ))
        }),
    }
}

pub fn sessions(ctx: &Ctx) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let rts = Runtimes::new(ctx, &cfg);
    let enabled = rts.enabled();
    let r = ListSessions { runtimes: &enabled }.list();
    let mut human = String::new();
    for s in &r.sessions {
        let _ = writeln!(
            human,
            "{:<6} {}{}",
            s.runtime.as_str(),
            s.name,
            if s.default { " (default)" } else { "" }
        );
    }
    Ok(Reply::new(&r, human.trim_end()))
}

pub fn agents(ctx: &Ctx) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let rts = Runtimes::new(ctx, &cfg);
    let enabled = rts.enabled();
    let r = ListAgents { runtimes: &enabled }.list();
    let mut human = String::new();
    for a in &r.agents {
        let status = serde_json::to_value(a.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = writeln!(
            human,
            "{:<10} {:<8} {:<9} {}",
            a.pane_id, a.agent, status, a.title
        );
    }
    Ok(Reply::new(&r, human.trim_end()))
}

pub fn inbox(ctx: &Ctx, since: Option<u64>, limit: Option<u32>) -> Result<Reply, CliError> {
    let st = open_state(ctx)?;
    let page = ListInbox { store: &st.store }.list(since, limit)?;
    let mut human = String::new();
    for e in &page.events {
        let status = if e.status == crate::features::events::domain::EventStatus::Pending {
            "pending"
        } else {
            "resolved"
        };
        let _ = writeln!(
            human,
            "{:>5} {} {:<17} {:<8} {}",
            e.seq,
            e.id,
            e.kind.as_str(),
            status,
            e.title
        );
    }
    Ok(Reply::new(&page, human.trim_end()))
}

pub fn respond(
    ctx: &Ctx,
    id: &str,
    action: ActionArg,
    text: Option<&str>,
) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let st = open_state(ctx)?;
    let rts = Runtimes::new(ctx, &cfg);
    let enabled = rts.enabled();
    let keymaps = EmbeddedKeymaps::load();
    let action = match action {
        ActionArg::Allow => Action::Allow,
        ActionArg::Deny => Action::Deny,
        ActionArg::Text => Action::Text,
    };
    with_notifier(ctx, &cfg, &st, |n| {
        let uc = RespondToEvent {
            store: &st.store,
            keymaps: &keymaps,
            clock: &SystemClock,
            runtimes: &enabled,
            notifier: n,
        };
        let out = uc.respond(id, action, text)?;
        Ok(Reply::new(
            &out,
            format!("{} {}", action.as_str(), out.event_id),
        ))
    })
}

pub fn devices_list(ctx: &Ctx) -> Result<Reply, CliError> {
    let st = open_state(ctx)?;
    let devices = DeviceAdmin {
        registry: &st.registry,
        keys: &st.keys,
    }
    .list()?;
    let mut human = String::new();
    for d in &devices {
        let _ = writeln!(
            human,
            "{} {}{}{}",
            d.id,
            d.name,
            if d.endpoint.is_empty() {
                if d.webpush {
                    ""
                } else {
                    " (no notifications)"
                }
            } else if d.endpoint_stale {
                " (endpoint stale)"
            } else {
                ""
            },
            if !d.webpush {
                ""
            } else if d.webpush_stale {
                " [webpush stale]"
            } else {
                " [webpush]"
            }
        );
    }
    Ok(Reply::new(&json!({"devices": devices}), human.trim_end()))
}

pub fn devices_revoke(ctx: &Ctx, id: &str) -> Result<Reply, CliError> {
    use crate::features::pairing::infrastructure::ssh_sessions::{ancestry, ProcSshSessions};
    use std::os::unix::fs::MetadataExt;
    let st = open_state(ctx)?;
    let proc_root = std::path::PathBuf::from("/proc");
    let sessions = ProcSshSessions {
        uid: std::fs::metadata(proc_root.join("self")).map_or(u32::MAX, |m| m.uid()),
        protect: ancestry(&proc_root, std::process::id()),
        proc_root,
        runner: &ctx.runner,
    };
    let r = DeviceAdmin {
        registry: &st.registry,
        keys: &st.keys,
    }
    .revoke(id, &sessions)?;
    let mut human = format!("Revoked {}", r.revoked.name);
    for c in &r.checklist {
        let _ = write!(human, "\n- {c}");
    }
    Ok(Reply::new(&r, human))
}

pub fn devices_set_endpoint(ctx: &Ctx, id: &str, url: &str) -> Result<Reply, CliError> {
    let st = open_state(ctx)?;
    let d = DeviceAdmin {
        registry: &st.registry,
        keys: &st.keys,
    }
    .set_endpoint(id, url)?;
    Ok(Reply::new(
        &json!({"device": d}),
        format!("Updated {}", d.name),
    ))
}

/// Reads one `set-webpush` JSON object from stdin and stores it.
pub fn devices_set_webpush(ctx: &Ctx, id: &str) -> Result<Reply, CliError> {
    let input = read_stdin(16 * 1024)?;
    let st = open_state(ctx)?;
    DeviceAdmin {
        registry: &st.registry,
        keys: &st.keys,
    }
    .set_webpush(id, &input)?;
    Ok(Reply::new(&json!({}), "Web Push registered"))
}

pub fn devices_clear_webpush(ctx: &Ctx, id: &str) -> Result<Reply, CliError> {
    let st = open_state(ctx)?;
    DeviceAdmin {
        registry: &st.registry,
        keys: &st.keys,
    }
    .clear_webpush(id)?;
    Ok(Reply::new(&json!({}), "Web Push removed"))
}

/// The directories (0700) and files (0600) that must stay private.
fn private_paths(ctx: &Ctx) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let p = &ctx.paths;
    (
        vec![p.config_dir.clone(), p.state_dir.clone()],
        vec![
            p.config_file(),
            p.devices_file(),
            p.host_key(),
            p.events_db(),
        ],
    )
}

/// Runs every check (read-only).
fn doctor_report(ctx: &Ctx) -> DoctorReport {
    let mut checks = Vec::new();
    let cfg = match ctx.config() {
        Ok(c) => {
            checks.push(Check::ok(
                "config",
                ctx.paths.config_file().display().to_string(),
            ));
            c
        }
        Err(e) => {
            checks.push(Check::fail("config", e.message));
            Config::default()
        }
    };
    let tailscale = TailscaleCli {
        runner: &ctx.runner,
        program: cfg.bins.tailscale.clone(),
    };
    checks.push(check_tailscale(&tailscale));
    checks.push(check_tailscale_ssh(&tailscale, cfg.ssh_port));
    let rts = Runtimes::new(ctx, &cfg);
    checks.extend(check_runtimes(
        &rts.enabled(),
        super::prereqs::herdr_install_hint(ctx).as_deref(),
    ));
    let p = &ctx.paths;
    let (dirs, files) = private_paths(ctx);
    checks.push(check_permissions(
        &dirs.iter().map(|d| d.as_path()).collect::<Vec<_>>(),
        &files.iter().map(|f| f.as_path()).collect::<Vec<_>>(),
    ));
    let host_key = check_ssh(&cfg.ssh_host_key_pub);
    checks.push(if super::prereqs::ssh_server_install(ctx).is_some() {
        host_key.with_fix(FixId::InstallSsh)
    } else {
        host_key
    });
    checks.push(super::prereqs::ssh_listening_check(ctx, cfg.ssh_port));
    let effective = sshd_password_logins(&ctx.runner, "sshd", ctx.user.as_deref());
    checks.push(check_sshd_auth(effective.as_deref()));
    checks.push(check_authorized_keys(
        effective.as_deref(),
        &ctx.paths.home,
        ctx.user.as_deref().unwrap_or_default(),
        std::fs::read_to_string(ctx.paths.authorized_keys())
            .ok()
            .as_deref(),
    ));
    checks.push(check_devices(&ctx.registry()));
    if let Some(c) = legacy_check(ctx) {
        checks.push(c);
    }
    checks.push(check_keymaps(EmbeddedKeymaps::load().all()));
    checks.push(if cfg!(target_os = "macos") {
        check_launchd_service(&p.launchd_plist())
    } else {
        check_systemd_service(&p.systemd_unit(), &ctx.runner, &cfg.bins.systemctl)
    });
    DoctorReport::new(checks)
}

/// The human doctor output.
fn render_report(report: &DoctorReport) -> String {
    let mut human = String::new();
    for c in &report.checks {
        let mark = match c.status {
            Status::Ok => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        };
        let _ = writeln!(human, "[{mark}] {:<13} {}", c.name, c.detail);
    }
    human.push_str(if report.healthy {
        "\nNext: open Falconer on your phone and add this host (falconer-hook pair)."
    } else {
        "\nFix the FAIL items above, then run `falconer-hook doctor` again."
    });
    human
}

pub fn doctor(ctx: &Ctx) -> Result<Reply, CliError> {
    let report = doctor_report(ctx);
    let human = render_report(&report);
    Ok(Reply::new(&report, human))
}

/// The fixes `doctor --fix` would apply now, in check order, each once.
pub fn pending_fixes(report: &DoctorReport) -> Vec<FixId> {
    let mut out: Vec<FixId> = Vec::new();
    for id in report.checks.iter().filter_map(Check::fix_id) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `doctor --fix [<id>]`: applies the pending fixes (or only `only`), each
/// through the same consent rules as `setup`, then checks again.
pub fn doctor_fix(ctx: &Ctx, only: Option<&str>, yes: bool, json: bool) -> Result<Reply, CliError> {
    let only = match only {
        None => None,
        Some(s) => Some(FixId::parse(s).ok_or_else(|| {
            let known: Vec<&str> = FixId::ALL.iter().map(|f| f.as_str()).collect();
            CliError::new(
                "unknown_fix",
                format!("unknown fix `{s}`; known fixes: {}", known.join(", ")),
            )
        })?),
    };
    let pending = pending_fixes(&doctor_report(ctx));
    let cfg = ctx.config().unwrap_or_default();
    let mode = super::prereqs::Mode {
        yes,
        interactive: super::lifecycle::interactive(json),
        ask: &super::lifecycle::ask,
    };
    let mut human = String::new();
    let mut fixes = Vec::new();
    let todo: Vec<FixId> = match only {
        Some(id) if !pending.contains(&id) => {
            let _ = writeln!(human, "Nothing to fix for {}.", id.as_str());
            fixes.push(json!({"id": id, "action": "not_needed"}));
            Vec::new()
        }
        Some(id) => vec![id],
        None => pending,
    };
    if todo.is_empty() && only.is_none() {
        let _ = writeln!(human, "Nothing to fix.");
    }
    for id in todo {
        let result = apply_fix(ctx, &cfg, id, &mode, &mut human);
        fixes.push(json!({"id": id, "action": result["action"], "result": result}));
    }
    let report = doctor_report(ctx);
    let _ = write!(human, "\n{}", render_report(&report));
    let data = json!({"healthy": report.healthy, "checks": report.checks, "fixes": fixes});
    Ok(Reply::new(&data, human.trim_start().to_string()))
}

/// Asks before a fix that is not a `setup` prerequisite; `PrintOnly` and
/// `Declined` are reported with `cmd` and nothing runs.
fn consented(
    mode: &super::prereqs::Mode,
    question: &str,
    cmd: &str,
    human: &mut String,
) -> Result<(), serde_json::Value> {
    match consent(mode.yes, mode.interactive, || (mode.ask)(question)) {
        Consent::Run => Ok(()),
        Consent::PrintOnly => {
            let _ = writeln!(human, "To fix, run: {cmd}");
            Err(json!({"action": "printed", "command": cmd}))
        }
        Consent::Declined => {
            let _ = writeln!(
                human,
                "Left as is (declined). Run `{cmd}` when you are ready."
            );
            Err(json!({"action": "declined", "command": cmd}))
        }
    }
}

/// Applies one fix by reusing the command that already does it. The result
/// always has an `action`.
pub fn apply_fix(
    ctx: &Ctx,
    cfg: &Config,
    id: FixId,
    mode: &super::prereqs::Mode,
    human: &mut String,
) -> serde_json::Value {
    use super::prereqs;
    match id {
        FixId::InstallRuntime => prereqs::ensure_runtime(ctx, cfg, mode, human),
        FixId::InstallTailscale | FixId::TailscaleUp => {
            prereqs::ensure_tailscale(ctx, cfg, mode, human)
        }
        FixId::InstallSsh | FixId::EnableSsh => prereqs::ensure_ssh(ctx, cfg, mode, human),
        FixId::FixPermissions => {
            let cmd = "chmod 700 the config and state directories, 600 their files";
            if let Err(v) = consented(
                mode,
                "Make the falconer-hook config and state private (chmod 700 dirs, 600 files)?",
                cmd,
                human,
            ) {
                return v;
            }
            let (dirs, files) = private_paths(ctx);
            let mut changed = Vec::new();
            let targets = dirs
                .iter()
                .map(|d| (d, 0o700))
                .chain(files.iter().map(|f| (f, 0o600)));
            for (path, want) in targets {
                let Ok(meta) = std::fs::metadata(path) else {
                    continue;
                };
                if meta.permissions().mode() & 0o077 == 0 {
                    continue;
                }
                if let Err(e) =
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(want))
                {
                    let e = format!("{}: {e}", path.display());
                    let _ = writeln!(human, "Could not fix permissions: {e}");
                    return json!({"action": "failed", "changed": changed, "error": e});
                }
                changed.push(path.display().to_string());
            }
            let _ = writeln!(human, "Made private: {}", changed.join(", "));
            json!({"action": "fixed", "changed": changed})
        }
        FixId::HardenSsh => {
            let cmd = "falconer-hook harden-ssh";
            if let Err(v) = consented(
                mode,
                "Turn off SSH password logins with `falconer-hook harden-ssh` (asks for sudo)?",
                cmd,
                human,
            ) {
                return v;
            }
            match super::harden::harden_ssh(ctx, false, true, false) {
                Ok(r) => {
                    let _ = writeln!(human, "{}", r.human);
                    json!({"action": "hardened", "data": r.data})
                }
                Err(e) => {
                    let _ = writeln!(human, "Could not harden SSH: {}", e.message);
                    json!({"action": "failed", "error": e.message})
                }
            }
        }
        FixId::InstallService => {
            let cmd = "falconer-hook service install";
            if let Err(v) = consented(
                mode,
                "Register the user service (falconer-hook service install)?",
                cmd,
                human,
            ) {
                return v;
            }
            match service(ctx, true, false) {
                Ok(r) => {
                    let _ = writeln!(human, "Installed the user service");
                    json!({"action": "installed", "plan": r.data})
                }
                Err(e) => {
                    let _ = writeln!(human, "Service not installed: {}", e.message);
                    json!({"action": "failed", "error": e.message})
                }
            }
        }
        FixId::Pair => {
            let _ = writeln!(human, "To pair a phone: {}", id.description());
            json!({"action": "manual", "description": id.description()})
        }
    }
}

pub fn service(ctx: &Ctx, install: bool, dry_run: bool) -> Result<Reply, CliError> {
    let cfg = ctx.config()?;
    let (platform, unit) = if cfg!(target_os = "macos") {
        (Platform::Launchd, ctx.paths.launchd_plist())
    } else {
        (Platform::Systemd, ctx.paths.systemd_unit())
    };
    let plan = if install {
        install_plan(
            platform,
            &ctx.exe,
            &ctx.paths.home,
            &unit,
            &cfg.bins.systemctl,
            &cfg.bins.launchctl,
        )?
    } else {
        uninstall_plan(platform, &unit, &cfg.bins.systemctl, &cfg.bins.launchctl)?
    };
    let mut human = String::new();
    // Upgrading from OpenMosh: stop and remove its service before ours starts.
    let legacy_unit = if cfg!(target_os = "macos") {
        ctx.paths.legacy_launchd_plist()
    } else {
        ctx.paths.legacy_systemd_unit()
    };
    let legacy = if install && legacy_unit.exists() {
        Some(legacy_uninstall_plan(
            platform,
            &legacy_unit,
            &cfg.bins.systemctl,
            &cfg.bins.launchctl,
        )?)
    } else {
        None
    };
    if let Some(l) = &legacy {
        for p in &l.remove {
            let _ = writeln!(human, "remove {} (OpenMosh)", p.display());
        }
        for c in &l.commands {
            let _ = writeln!(human, "run    {}", c.join(" "));
        }
    }
    for (p, _) in &plan.write {
        let _ = writeln!(human, "write  {}", p.display());
    }
    for p in &plan.remove {
        let _ = writeln!(human, "remove {}", p.display());
    }
    for c in &plan.commands {
        let _ = writeln!(human, "run    {}", c.join(" "));
    }
    if !dry_run {
        let apply = ApplyPlan {
            files: &FsServiceFiles,
            runner: &ctx.runner,
        };
        if let Some(l) = &legacy {
            apply.apply(l)?;
        }
        apply.apply(&plan)?;
    }
    let mut data = json!({
        "platform": plan.platform,
        "dry_run": dry_run,
        "unit": unit,
        "contents": plan.write.first().map(|(_, c)| c.clone()),
        "commands": plan.commands,
    });
    if let Some(l) = legacy {
        data["replaced_legacy"] = json!({"unit": legacy_unit, "commands": l.commands});
    }
    Ok(Reply::new(&data, human.trim_end()))
}

/// What an OpenMosh install (the name before Falconer) left behind; `None`
/// when nothing. Read-only.
fn legacy_check(ctx: &Ctx) -> Option<Check> {
    let p = &ctx.paths;
    let mut left: Vec<String> = Vec::new();
    for unit in [p.legacy_systemd_unit(), p.legacy_launchd_plist()] {
        if unit.exists() {
            left.push(format!("service {}", unit.display()));
        }
    }
    let agents: Vec<&str> = super::integrations::legacy_agents(ctx)
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    if !agents.is_empty() {
        left.push(format!(
            "hooks calling openmosh-hook ({})",
            agents.join(", ")
        ));
    }
    let mut detail = String::new();
    if !left.is_empty() {
        detail = format!("{}; run `falconer-hook setup`", left.join(", "));
    }
    let dirs = p.legacy_dirs_left();
    if !dirs.is_empty() {
        if !detail.is_empty() {
            detail.push_str("; ");
        }
        let _ = write!(
            detail,
            "{} kept because the Falconer directory already exists: merge or remove it by hand",
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    (!detail.is_empty()).then(|| Check::warn("legacy", format!("OpenMosh leftovers: {detail}")))
}

pub fn version() -> Reply {
    Reply::new(
        &json!({"version": env!("CARGO_PKG_VERSION"), "protocol": {"pair": 1, "push": 1, "herdr": 1}}),
        format!("falconer-hook {}", env!("CARGO_PKG_VERSION")),
    )
}
