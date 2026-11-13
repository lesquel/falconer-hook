//! Short request/response commands.

use serde_json::json;
use std::fmt::Write as _;
use std::io::Read;

use super::{open_state, with_notifier, Ctx, Runtimes};
use crate::cli::output::{CliError, Reply};
use crate::cli::{ActionArg, KindArg};
use crate::features::agents::application::{ListAgents, ListSessions};
use crate::features::doctor::application::{
    check_devices, check_keymaps, check_runtimes, check_tailscale, check_tailscale_ssh,
};
use crate::features::doctor::domain::{Check, DoctorReport, Status};
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
use crate::features::service::domain::{install_plan, uninstall_plan, Platform};
use crate::features::service::infrastructure::FsServiceFiles;
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
            "{} {}{}",
            d.id,
            d.name,
            if d.endpoint.is_empty() {
                " (no notifications)"
            } else if d.endpoint_stale {
                " (endpoint stale)"
            } else {
                ""
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

pub fn doctor(ctx: &Ctx) -> Result<Reply, CliError> {
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
    checks.extend(check_runtimes(&rts.enabled()));
    let p = &ctx.paths;
    let files = [
        p.config_file(),
        p.devices_file(),
        p.host_key(),
        p.events_db(),
    ];
    checks.push(check_permissions(
        &[&p.config_dir, &p.state_dir],
        &files.iter().map(|f| f.as_path()).collect::<Vec<_>>(),
    ));
    checks.push(check_ssh(&cfg.ssh_host_key_pub));
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
    checks.push(check_keymaps(EmbeddedKeymaps::load().all()));
    checks.push(if cfg!(target_os = "macos") {
        check_launchd_service(&p.launchd_plist())
    } else {
        check_systemd_service(&p.systemd_unit(), &ctx.runner, &cfg.bins.systemctl)
    });
    let report = DoctorReport::new(checks);
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
        "\nNext: open OpenMosh on your phone and add this host (openmosh-hook pair)."
    } else {
        "\nFix the FAIL items above, then run `openmosh-hook doctor` again."
    });
    Ok(Reply::new(&report, human))
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
        ApplyPlan {
            files: &FsServiceFiles,
            runner: &ctx.runner,
        }
        .apply(&plan)?;
    }
    let data = json!({
        "platform": plan.platform,
        "dry_run": dry_run,
        "unit": unit,
        "contents": plan.write.first().map(|(_, c)| c.clone()),
        "commands": plan.commands,
    });
    Ok(Reply::new(&data, human.trim_end()))
}

pub fn version() -> Reply {
    Reply::new(
        &json!({"version": env!("CARGO_PKG_VERSION"), "protocol": {"pair": 1, "push": 1, "herdr": 1}}),
        format!("openmosh-hook {}", env!("CARGO_PKG_VERSION")),
    )
}
