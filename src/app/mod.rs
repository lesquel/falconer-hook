//! Composition root (with `main.rs`): the only place that knows concrete
//! adapter types. Each handler builds adapters, injects them into use cases
//! and returns a `Reply` for the CLI envelope.

mod attach;
mod commands;
mod harden;
mod integrations;
mod lifecycle;
mod pair;
mod serve;
pub mod tui;
mod usage;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::cli::output::{CliError, Reply};
use crate::cli::Command;
use crate::features::agents::domain::AgentRuntime;
use crate::features::agents::infrastructure::herdr::HerdrRuntime;
use crate::features::agents::infrastructure::tmux::TmuxRuntime;
use crate::features::events::domain::{valid_pane_id, PaneRef, Runtime};
use crate::features::events::infrastructure::sqlite_store::SqliteEventStore;
use crate::features::pairing::infrastructure::authorized_keys::FileAuthorizedKeys;
use crate::features::pairing::infrastructure::toml_registry::TomlDeviceRegistry;
use crate::shared::config::{Config, Paths};
use crate::shared::domain::runtime::valid_session;
use crate::shared::infrastructure::fs_secure::{check_private_file_if_exists, ensure_private_dir};
use crate::shared::infrastructure::host_identity::HostIdentity;
use crate::shared::infrastructure::process::ProcessRunner;
use crate::shared::infrastructure::system::hostname;

/// Process environment captured once in `main`.
pub struct Ctx {
    pub paths: Paths,
    pub runner: ProcessRunner,
    pub user: Option<String>,
    /// Agent pane from `$HERDR_ENV`/`$HERDR_PANE_ID` or `$TMUX_PANE`.
    pub pane: Option<PaneRef>,
    pub exe: PathBuf,
    /// `$PATH`, used only to detect installed agents.
    pub path_env: Option<String>,
    pub stop: Arc<AtomicBool>,
}

impl Ctx {
    pub fn from_env() -> Result<Self, CliError> {
        let get = |k: &str| std::env::var(k).ok();
        let paths = Paths::from_env(get)?;
        let pane = pane_from_env(get);
        let exe = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .unwrap_or_else(|_| PathBuf::from("openmosh-hook"));
        Ok(Self {
            paths,
            runner: ProcessRunner::default(),
            user: get("USER").filter(|u| {
                !u.is_empty()
                    && u.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }),
            pane,
            exe,
            path_env: get("PATH"),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn config(&self) -> Result<Config, CliError> {
        Ok(Config::load(&self.paths.config_file())?)
    }

    pub fn host_alias(&self, cfg: &Config) -> String {
        cfg.host_alias
            .as_deref()
            .map(|a| {
                a.chars()
                    .filter(|c| !c.is_control())
                    .take(64)
                    .collect::<String>()
            })
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| hostname(&self.runner))
    }

    pub fn lock_path(&self) -> PathBuf {
        self.paths.state_dir.join(".lock")
    }

    pub fn registry(&self) -> TomlDeviceRegistry {
        TomlDeviceRegistry {
            path: self.paths.devices_file(),
            lock_path: self.lock_path(),
        }
    }
}

/// The agent pane this process runs in, with the server it belongs to:
/// `$HERDR_PANE_ID` (+ `$HERDR_SESSION`, or the session named by
/// `$HERDR_SOCKET_PATH`), else `$TMUX_PANE` (+ the socket from `$TMUX`).
pub fn pane_from_env(get: impl Fn(&str) -> Option<String>) -> Option<PaneRef> {
    if get("HERDR_ENV").as_deref() == Some("1") {
        if let Some(id) = get("HERDR_PANE_ID").filter(|p| valid_pane_id(p)) {
            let mut p = PaneRef::new(Runtime::Herdr, id);
            p.context.session = get("HERDR_SESSION")
                .or_else(|| get("HERDR_SOCKET_PATH").and_then(|s| herdr_session_of(&s)))
                .filter(|s| valid_session(Runtime::Herdr, s));
            return Some(p);
        }
    }
    let id = get("TMUX_PANE").filter(|p| valid_pane_id(p))?;
    let mut p = PaneRef::new(Runtime::Tmux, id);
    // `$TMUX` is `<socket>,<server pid>,<session index>`.
    p.context.session = get("TMUX")
        .and_then(|t| t.split(',').next().map(str::to_string))
        .filter(|s| valid_session(Runtime::Tmux, s));
    Some(p)
}

/// `…/herdr/sessions/<name>/herdr.sock` → `<name>`.
fn herdr_session_of(socket: &str) -> Option<String> {
    let mut parts = socket.rsplit('/');
    let (file, name, dir) = (parts.next()?, parts.next()?, parts.next()?);
    (file == "herdr.sock" && dir == "sessions").then(|| name.to_string())
}

/// Private state, opened only after permissions were enforced (control T-8).
pub struct State {
    pub identity: HostIdentity,
    pub store: SqliteEventStore,
    pub registry: TomlDeviceRegistry,
    pub keys: FileAuthorizedKeys,
}

pub fn open_state(ctx: &Ctx) -> Result<State, CliError> {
    ensure_private_dir(&ctx.paths.config_dir)?;
    ensure_private_dir(&ctx.paths.state_dir)?;
    for f in [
        ctx.paths.devices_file(),
        ctx.paths.host_key(),
        ctx.paths.events_db(),
        ctx.paths.config_file(),
    ] {
        check_private_file_if_exists(&f)?;
    }
    Ok(State {
        identity: HostIdentity::load_or_create(&ctx.paths.host_key())?,
        store: SqliteEventStore::open(&ctx.paths.events_db())?,
        registry: ctx.registry(),
        keys: FileAuthorizedKeys {
            path: ctx.paths.authorized_keys(),
            lock_path: ctx.lock_path(),
        },
    })
}

/// Herdr and tmux adapters for the enabled sources.
pub struct Runtimes<'a> {
    pub herdr: HerdrRuntime<'a>,
    pub tmux: TmuxRuntime<'a>,
    herdr_on: bool,
    tmux_on: bool,
}

impl<'a> Runtimes<'a> {
    pub fn new(ctx: &'a Ctx, cfg: &Config) -> Self {
        Self {
            herdr: HerdrRuntime {
                runner: &ctx.runner,
                program: cfg.bins.herdr.clone(),
            },
            tmux: TmuxRuntime {
                runner: &ctx.runner,
                program: cfg.bins.tmux.clone(),
            },
            herdr_on: cfg.source_enabled("herdr"),
            tmux_on: cfg.source_enabled("tmux"),
        }
    }

    pub fn enabled(&self) -> Vec<&dyn AgentRuntime> {
        let mut v: Vec<&dyn AgentRuntime> = Vec::new();
        if self.herdr_on {
            v.push(&self.herdr);
        }
        if self.tmux_on {
            v.push(&self.tmux);
        }
        v
    }
}

pub use attach::attach_herdr;
pub use tui::run_tui;

/// Dispatches one command.
pub fn run(ctx: &Ctx, command: Command) -> Result<Reply, CliError> {
    use crate::cli::{DevicesCmd, InboxCmd, IntegrationCmd, ListCmd, ServiceCmd};
    match command {
        Command::Serve => serve::serve(ctx),
        Command::Emit { source, kind, .. } => commands::emit(ctx, &source, kind),
        Command::Pair {
            alias,
            timeout,
            invert,
            uri,
        } => pair::pair(ctx, alias, timeout, invert, uri),
        Command::PairSsh { .. } => pair::pair_ssh(ctx),
        Command::Sessions {
            cmd: ListCmd::List { .. },
        } => commands::sessions(ctx),
        Command::Agents {
            cmd: ListCmd::List { .. },
        } => commands::agents(ctx),
        Command::Inbox {
            cmd: InboxCmd::List { since, limit, .. },
        } => commands::inbox(ctx, since, limit),
        Command::Respond {
            event_id,
            action,
            text,
            ..
        } => commands::respond(ctx, &event_id, action, text.as_deref()),
        Command::Devices { cmd } => match cmd {
            DevicesCmd::List { .. } => commands::devices_list(ctx),
            DevicesCmd::Revoke { id, .. } => commands::devices_revoke(ctx, &id),
            DevicesCmd::SetEndpoint { id, url, .. } => {
                commands::devices_set_endpoint(ctx, &id, &url)
            }
        },
        Command::Integration { cmd } => match cmd {
            IntegrationCmd::Install {
                agent, settings, ..
            } => integrations::integration(ctx, agent, settings, true),
            IntegrationCmd::Uninstall {
                agent, settings, ..
            } => integrations::integration(ctx, agent, settings, false),
            IntegrationCmd::List { .. } => integrations::list(ctx),
        },
        Command::Setup {
            yes,
            no_service,
            out,
        } => lifecycle::setup(ctx, yes, no_service, out.json),
        Command::Uninstall {
            all,
            dry_run,
            yes,
            out,
        } => lifecycle::uninstall(ctx, all, dry_run, yes, out.json),
        Command::Doctor { .. } => commands::doctor(ctx),
        Command::Usage { days, .. } => usage::usage(ctx, days),
        Command::HardenSsh { dry_run, yes, out } => harden::harden_ssh(ctx, dry_run, yes, out.json),
        Command::Service { cmd } => match cmd {
            ServiceCmd::Install { dry_run, .. } => commands::service(ctx, true, dry_run),
            ServiceCmd::Uninstall { dry_run, .. } => commands::service(ctx, false, dry_run),
        },
        Command::Attach { .. } | Command::Tui => {
            Err(CliError::new("internal", "dispatched by main"))
        }
        Command::Version { .. } => Ok(commands::version()),
    }
}

/// Builds the push notifier (sealer + HTTPS publisher) for the duration of `f`.
pub fn with_notifier<T>(
    ctx: &Ctx,
    cfg: &Config,
    st: &State,
    f: impl FnOnce(&dyn crate::features::events::domain::EventNotifier) -> Result<T, CliError>,
) -> Result<T, CliError> {
    use crate::features::push::application::PublishEvent;
    use crate::features::push::infrastructure::notifier::PushNotifier;
    use crate::features::push::infrastructure::ntfy_publisher::HttpPublisher;
    use crate::features::push::infrastructure::xchacha_sealer::XChaChaSealer;

    let sealer = XChaChaSealer::new(st.identity.secret());
    let publisher =
        HttpPublisher::new().map_err(|_| CliError::new("internal", "HTTP client unavailable"))?;
    let notifier = PushNotifier {
        publish: PublishEvent {
            registry: &st.registry,
            sealer: &sealer,
            publisher: &publisher,
        },
        host_alias: ctx.host_alias(cfg),
    };
    f(&notifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn pane_context_comes_from_the_environment() {
        let p = pane_from_env(env(&[
            ("TMUX", "/tmp/tmux-1000/work,4120,0"),
            ("TMUX_PANE", "%7"),
        ]))
        .unwrap();
        assert_eq!((p.runtime, p.pane.as_str()), (Runtime::Tmux, "%7"));
        assert_eq!(p.context.session.as_deref(), Some("/tmp/tmux-1000/work"));
        let p = pane_from_env(env(&[("TMUX", "relative,1,0"), ("TMUX_PANE", "%7")])).unwrap();
        assert_eq!(p.context.session, None, "only absolute sockets");

        let p = pane_from_env(env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p2"),
            ("HERDR_SESSION", "side"),
        ]))
        .unwrap();
        assert_eq!(p.context.session.as_deref(), Some("side"));
        let p = pane_from_env(env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p2"),
            (
                "HERDR_SOCKET_PATH",
                "/h/.config/herdr/sessions/work/herdr.sock",
            ),
        ]))
        .unwrap();
        assert_eq!(p.context.session.as_deref(), Some("work"));
        let p = pane_from_env(env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p2"),
            ("HERDR_SESSION", "--bad"),
        ]))
        .unwrap();
        assert_eq!(p.context.session, None);
        assert!(pane_from_env(env(&[])).is_none());
    }
}
