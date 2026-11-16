//! `integration install|uninstall|list`: wires the agent catalog to files.

use serde::Serialize;
use std::fmt::Write as _;
use std::path::PathBuf;

use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::cli::AgentArg;
use crate::features::integration::application::SettingsFile;
use crate::features::integration::application::{Integration, IntegrationResult};
use crate::features::integration::domain::owned;
use crate::features::integration::domain::{AgentId, AgentSpec, Base, Format, CATALOG};
use crate::features::integration::infrastructure::JsonSettingsFile;
use crate::shared::infrastructure::system::on_path;

pub fn agent_id(a: AgentArg) -> AgentId {
    match a {
        AgentArg::Claude => AgentId::Claude,
        AgentArg::Codex => AgentId::Codex,
        AgentArg::Gemini => AgentId::Gemini,
        AgentArg::Cursor => AgentId::Cursor,
        AgentArg::Copilot => AgentId::Copilot,
        AgentArg::Kiro => AgentId::Kiro,
        AgentArg::Opencode => AgentId::Opencode,
        AgentArg::Aider => AgentId::Aider,
    }
}

fn base(ctx: &Ctx, b: Base) -> PathBuf {
    match b {
        Base::Home => ctx.paths.home.clone(),
        Base::XdgConfig => ctx.paths.xdg_config_home.clone(),
    }
}

pub fn default_file(ctx: &Ctx, spec: &AgentSpec) -> PathBuf {
    base(ctx, spec.base).join(spec.rel)
}

pub fn detected(ctx: &Ctx, spec: &AgentSpec) -> bool {
    spec.bins
        .iter()
        .any(|b| on_path(b, ctx.path_env.as_deref()))
        || spec
            .detect
            .iter()
            .any(|(b, rel)| base(ctx, *b).join(rel).exists())
}

pub fn exe(ctx: &Ctx) -> Result<String, CliError> {
    ctx.exe
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| CliError::new("invalid_input", "executable path is not UTF-8"))
}

/// Installs or removes one agent's integration.
pub fn apply(
    ctx: &Ctx,
    id: AgentId,
    settings: Option<PathBuf>,
    install: bool,
) -> Result<IntegrationResult, CliError> {
    let spec = id.spec();
    let default = settings.is_none();
    let file = JsonSettingsFile {
        path: settings.unwrap_or_else(|| default_file(ctx, spec)),
    };
    let uc = Integration {
        file: &file,
        spec,
        exe: exe(ctx)?,
    };
    let mut r = if install {
        uc.install()?
    } else {
        uc.uninstall()?
    };
    if let (true, Some(legacy)) = (default, legacy_file(ctx, spec)) {
        r.changed |= uc.remove_legacy_file(&legacy)?;
    }
    Ok(r)
}

/// The file an OpenMosh install created for this agent under its old name.
fn legacy_file(ctx: &Ctx, spec: &AgentSpec) -> Option<JsonSettingsFile> {
    spec.legacy_rel.map(|rel| JsonSettingsFile {
        path: base(ctx, spec.base).join(rel),
    })
}

/// Agents whose hooks still call `openmosh-hook` (or whose OpenMosh-named
/// file is still there): left over from before the rename.
pub fn legacy_agents(ctx: &Ctx) -> Vec<&'static AgentSpec> {
    let Ok(exe) = exe(ctx) else {
        return Vec::new();
    };
    CATALOG
        .iter()
        .filter(|spec| {
            let file = JsonSettingsFile {
                path: default_file(ctx, spec),
            };
            let uc = Integration {
                file: &file,
                spec,
                exe: exe.clone(),
            };
            uc.has_legacy().unwrap_or(false)
                || legacy_file(ctx, spec).is_some_and(|l| {
                    let Format::Owned(kind) = spec.format else {
                        return false;
                    };
                    l.load()
                        .ok()
                        .flatten()
                        .is_some_and(|t| owned::is_ours(kind, &t))
                })
        })
        .collect()
}

/// Rewrites every agent's OpenMosh hooks to call this executable: old
/// entries are replaced in place (never duplicated), OpenMosh-named files
/// are swapped for the Falconer ones, everything else is left alone.
pub fn migrate_legacy(ctx: &Ctx) -> Vec<serde_json::Value> {
    legacy_agents(ctx)
        .into_iter()
        .map(|spec| match apply(ctx, spec.id, None, true) {
            Ok(r) => serde_json::json!({"agent": r.agent, "settings": r.settings, "action": "migrated"}),
            Err(e) => serde_json::json!({"agent": spec.id.as_str(), "action": "failed", "error": e.message}),
        })
        .collect()
}

pub fn describe(r: &IntegrationResult, install: bool) -> String {
    let name = CATALOG
        .iter()
        .find(|s| s.id.as_str() == r.agent)
        .map_or(r.agent, |s| s.name);
    // Same wording as the original Claude-only command.
    let mut s = match (install, r.changed) {
        (true, true) => format!("Installed {name} hooks in {}", r.settings),
        (true, false) => format!("{name} hooks already present in {}", r.settings),
        (false, true) => format!("Removed {name} hooks from {}", r.settings),
        (false, false) => format!("No {name} hooks to remove in {}", r.settings),
    };
    if let Some(n) = r.note {
        let _ = write!(s, "\nNote: {n}");
    }
    s
}

pub fn integration(
    ctx: &Ctx,
    agent: AgentArg,
    settings: Option<PathBuf>,
    install: bool,
) -> Result<Reply, CliError> {
    let r = apply(ctx, agent_id(agent), settings, install)?;
    let human = describe(&r, install);
    Ok(Reply::new(&r, human))
}

#[derive(Debug, Serialize)]
pub struct AgentStatus {
    pub agent: &'static str,
    pub name: &'static str,
    pub detected: bool,
    /// `None` when the file could not be read or parsed (see `error`).
    pub installed: Option<bool>,
    pub settings: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub doc: &'static str,
}

pub fn status(ctx: &Ctx, spec: &'static AgentSpec) -> AgentStatus {
    let path = default_file(ctx, spec);
    let file = JsonSettingsFile { path: path.clone() };
    let installed = exe(ctx).and_then(|exe| {
        Integration {
            file: &file,
            spec,
            exe,
        }
        .installed()
        .map_err(CliError::from)
    });
    AgentStatus {
        agent: spec.id.as_str(),
        name: spec.name,
        detected: detected(ctx, spec),
        installed: installed.as_ref().ok().copied(),
        settings: path.display().to_string(),
        error: installed.err().map(|e| e.message),
        doc: spec.doc,
    }
}

pub fn statuses(ctx: &Ctx) -> Vec<AgentStatus> {
    CATALOG.iter().map(|s| status(ctx, s)).collect()
}

pub fn list(ctx: &Ctx) -> Result<Reply, CliError> {
    let agents = statuses(ctx);
    let mut human = String::new();
    for a in &agents {
        let installed = match a.installed {
            Some(true) => "installed",
            Some(false) => "-",
            None => "error",
        };
        let _ = writeln!(
            human,
            "{:<9} {:<19} {:<12} {:<9} {}",
            a.agent,
            a.name,
            if a.detected { "detected" } else { "not found" },
            installed,
            a.settings
        );
    }
    Ok(Reply::new(
        &serde_json::json!({"agents": agents}),
        human.trim_end(),
    ))
}
