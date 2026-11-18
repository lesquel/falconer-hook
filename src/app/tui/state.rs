//! TUI state machine: screens, selection and confirmation gating.
//!
//! Every action is a `cli::Command` run through the `Exec` port, so the TUI
//! uses exactly the same use cases as the CLI and is testable without a
//! terminal. Commands that run installers, sudo or a login (fixes, setup
//! with `--yes`, update, SSH hardening, pairing) are returned as an `Effect`
//! for the terminal loop, which suspends the TUI to run them.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

use super::actions;
pub use super::actions::Act;
use super::i18n::{t, tf, Lang};
pub use super::wizard::{StepStatus, WizardStep};
use crate::cli::output::{CliError, Reply};
use crate::cli::{
    ActionArg, AgentArg, Command, DevicesCmd, InboxCmd, IntegrationCmd, JsonFlag, ListCmd,
    ServiceCmd,
};
pub use crate::features::doctor::domain::FixId;
use crate::features::setup::domain::prereqs::{HERDR_INSTALLER_URL, TAILSCALE_INSTALLER_URL};
use crate::features::update::domain::script_url;

/// Runs one CLI command (the composition root in production, a fake in tests).
pub trait Exec {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError>;
    /// Runs `cmd` with `input` on its stdin (`devices set-webpush` reads it).
    fn exec_stdin(&mut self, cmd: Command, input: &str) -> Result<Reply, CliError>;
}

/// Word the user must type to confirm `uninstall`.
pub const UNINSTALL_WORD: &str = "uninstall";
const MAX_TEXT: usize = 4096;
/// `usage --days` choices.
pub const USAGE_DAYS: [i64; 4] = [1, 7, 14, 31];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Agents,
    Phones,
    Integrations,
    Service,
    Settings,
    Usage,
    Advanced,
}

impl Screen {
    pub const ALL: [Screen; 8] = [
        Screen::Home,
        Screen::Agents,
        Screen::Phones,
        Screen::Integrations,
        Screen::Service,
        Screen::Settings,
        Screen::Usage,
        Screen::Advanced,
    ];

    /// Short name for the screen list.
    pub fn tab(self) -> &'static str {
        match self {
            Screen::Home => "Home",
            Screen::Agents => "Agents",
            Screen::Phones => "Phones",
            Screen::Integrations => "Integrations",
            Screen::Service => "Service",
            Screen::Settings => "Settings",
            Screen::Usage => "Usage",
            Screen::Advanced => "Advanced",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Agents => "Agents & inbox",
            s => s.tab(),
        }
    }

    pub fn index(self) -> usize {
        Screen::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Install,
    Restart,
    Uninstall,
}

/// An action waiting for `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Respond {
        event_id: String,
        title: String,
        action: ActionArg,
        text: Option<String>,
    },
    Revoke {
        id: String,
        name: String,
    },
    Integration {
        agent: AgentArg,
        name: String,
        install: bool,
    },
    Setup {
        yes: bool,
    },
    LegacyCleanup,
    Fix {
        id: FixId,
    },
    FixAll {
        ids: Vec<FixId>,
    },
    Update {
        tag: String,
    },
    Service(ServiceOp),
    SetEndpoint {
        id: String,
        name: String,
        url: String,
    },
    SetWebpush {
        id: String,
        name: String,
        subscription: String,
    },
    ClearWebpush {
        id: String,
        name: String,
    },
    /// Wizard: install the integration of every detected agent.
    InstallDetected {
        agents: Vec<(AgentArg, String)>,
    },
}

/// What a fix does, for the Home list.
pub fn fix_title(lang: Lang, id: FixId) -> &'static str {
    t(
        lang,
        match id {
            FixId::InstallRuntime => "Install Herdr or tmux",
            FixId::InstallTailscale => "Install Tailscale",
            FixId::TailscaleUp => "Start Tailscale (sudo tailscale up)",
            FixId::InstallSsh => "Install the OpenSSH server",
            FixId::EnableSsh => "Turn the SSH server on",
            FixId::FixPermissions => "Make the config and state private",
            FixId::HardenSsh => "SSH: keys only, no passwords",
            FixId::InstallService => "Install the user service",
            FixId::Pair => "Pair a phone",
        },
    )
}

/// The exact source or command a fix runs, shown before asking.
fn fix_detail(lang: Lang, id: FixId) -> String {
    match id {
        FixId::InstallRuntime => tf(
            lang,
            "Homebrew or your package manager when present, else the official installer {}, downloaded over HTTPS to a private file and run with sh.",
            &[HERDR_INSTALLER_URL],
        ),
        FixId::InstallTailscale => tf(
            lang,
            "Homebrew when present, else the official installer {}, downloaded over HTTPS to a private file and run with sh.",
            &[TAILSCALE_INSTALLER_URL],
        ),
        FixId::TailscaleUp => {
            t(lang, "Runs `sudo tailscale up`; open the login URL it prints.").into()
        }
        FixId::InstallSsh => t(
            lang,
            "Installs the OpenSSH server with your package manager (sudo), then turns it on.",
        )
        .into(),
        FixId::EnableSsh => t(
            lang,
            "Runs `sudo systemctl enable --now ssh` (macOS: `sudo systemsetup -setremotelogin on`).",
        )
        .into(),
        FixId::FixPermissions => t(
            lang,
            "chmod 700 the config and state directories, 600 their files.",
        )
        .into(),
        FixId::HardenSsh => t(
            lang,
            "Runs `falconer-hook harden-ssh`; sudo asks for your password.",
        )
        .into(),
        FixId::InstallService => t(lang, "Runs `falconer-hook service install`.").into(),
        FixId::Pair => String::new(),
    }
}

impl Pending {
    pub fn question(&self, lang: Lang) -> String {
        let in_terminal = t(
            lang,
            "It runs in this terminal; the TUI comes back when it ends.",
        );
        match self {
            Pending::Respond {
                title,
                action,
                text,
                ..
            } => match action {
                ActionArg::Allow => tf(lang, "Allow \"{}\"?", &[title]),
                ActionArg::Deny => tf(lang, "Deny \"{}\"?", &[title]),
                ActionArg::Text => tf(
                    lang,
                    "Send \"{}\" to \"{}\"?",
                    &[text.as_deref().unwrap_or_default(), title],
                ),
            },
            Pending::Revoke { name, .. } => tf(
                lang,
                "Revoke {}? Its SSH key and push registration are removed.",
                &[name],
            ),
            Pending::Integration { name, install, .. } => tf(
                lang,
                if *install {
                    "Install the {} integration?"
                } else {
                    "Uninstall the {} integration?"
                },
                &[name],
            ),
            Pending::Setup { yes: false } => t(
                lang,
                "Run setup (config, user service, doctor)? Integrations are left as they are and nothing is installed.",
            )
            .into(),
            Pending::Setup { yes: true } => format!(
                "{}\n{}\n{in_terminal}",
                t(
                    lang,
                    "Run setup --yes: install every detected integration and every missing prerequisite without asking again?"
                ),
                tf(
                    lang,
                    "Official installers: {} and {} (Homebrew or your package manager first); sudo may ask for your password.",
                    &[HERDR_INSTALLER_URL, TAILSCALE_INSTALLER_URL],
                )
            ),
            Pending::LegacyCleanup => t(
                lang,
                "Clean up OpenMosh leftovers? Runs setup, which moves old hooks and replaces the old service.",
            )
            .into(),
            Pending::Fix {
                id: FixId::FixPermissions,
            } => format!(
                "{}?\n{}",
                fix_title(lang, FixId::FixPermissions),
                fix_detail(lang, FixId::FixPermissions)
            ),
            Pending::Fix { id } => format!(
                "{}?\n{}\n{in_terminal}",
                fix_title(lang, *id),
                fix_detail(lang, *id)
            ),
            Pending::FixAll { ids } => format!(
                "{}\n{}\n{in_terminal}",
                tf(
                    lang,
                    "Fix {} problems: {}?",
                    &[
                        &ids.len().to_string(),
                        &ids.iter()
                            .map(|i| fix_title(lang, *i))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ],
                ),
                t(lang, "Each fix shows its exact command and asks first."),
            ),
            Pending::Update { tag } => format!(
                "{}\n{}\n{in_terminal}",
                tf(lang, "Update Falconer to {}?", &[tag]),
                tf(
                    lang,
                    "Downloads {} and runs it; it verifies the signed release with minisign and keeps your config and service.",
                    &[&script_url(tag)],
                ),
            ),
            Pending::Service(op) => t(
                lang,
                match op {
                    ServiceOp::Install => "Install (or reinstall) the user service?",
                    ServiceOp::Restart => "Restart the user service?",
                    ServiceOp::Uninstall => {
                        "Uninstall the user service? Phones stop getting notifications."
                    }
                },
            )
            .into(),
            Pending::SetEndpoint { name, url, .. } => {
                tf(lang, "Set the push endpoint of {} to {}?", &[name, url])
            }
            Pending::SetWebpush { name, .. } => {
                tf(lang, "Register this Web Push subscription for {}?", &[name])
            }
            Pending::ClearWebpush { name, .. } => {
                tf(lang, "Remove the Web Push subscription of {}?", &[name])
            }
            Pending::InstallDetected { agents } => tf(
                lang,
                "Install the integration of every detected agent: {}?",
                &[&agents
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")],
            ),
        }
    }
}

/// What a text field is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Reply { event_id: String, title: String },
    Webpush { id: String, name: String },
    Endpoint { id: String, name: String },
}

impl Input {
    fn max(&self) -> usize {
        match self {
            Input::Reply { .. } => MAX_TEXT,
            Input::Webpush { .. } => 16 * 1024,
            Input::Endpoint { .. } => 2048,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Confirm(Pending),
    /// Uninstall: the user must type `uninstall`.
    TypeConfirm {
        input: String,
    },
    /// Free text, confirmed afterwards.
    TextInput {
        purpose: Input,
        input: String,
    },
}

/// What the terminal loop must do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    /// Suspend the TUI and run the QR pairing flow in the terminal.
    Pair {
        invert: bool,
    },
    /// The rest suspend the TUI and run `command()` in the terminal.
    HardenSsh,
    Fix {
        id: FixId,
    },
    FixAll,
    Update {
        tag: String,
    },
    SetupAll,
}

impl Effect {
    /// The command to run in the plain terminal, for the suspending effects
    /// other than pairing. Prompts and sudo talk to the person there.
    pub fn command(&self) -> Option<Command> {
        let human = JsonFlag { json: false };
        Some(match self {
            Effect::None | Effect::Quit | Effect::Pair { .. } => return None,
            Effect::HardenSsh => Command::HardenSsh {
                dry_run: false,
                yes: false,
                out: human,
            },
            // The TUI already showed the fix and asked.
            Effect::Fix { id } => Command::Doctor {
                fix: Some(Some(id.as_str().into())),
                yes: true,
                out: human,
            },
            // Each fix shows its command and asks in the terminal.
            Effect::FixAll => Command::Doctor {
                fix: Some(None),
                yes: false,
                out: human,
            },
            Effect::Update { tag } => Command::Update {
                check: false,
                version: Some(tag.clone()),
                yes: true,
                out: human,
            },
            Effect::SetupAll => Command::Setup {
                yes: true,
                no_service: false,
                out: human,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    pub name: String,
    pub status: String,
    pub detail: String,
    pub fix: Option<FixId>,
}

impl CheckRow {
    pub fn ok(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub pane: String,
    pub agent: String,
    pub status: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: String,
    pub seq: u64,
    pub agent: String,
    pub kind: String,
    pub pending: bool,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub stale: bool,
    pub ssh: bool,
    /// Paired without a push endpoint: no notifications.
    pub no_push: bool,
    pub endpoint: String,
    pub webpush: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRow {
    pub agent: String,
    pub name: String,
    pub detected: bool,
    pub installed: Option<bool>,
    pub settings: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub error: bool,
    pub text: String,
}

/// The background `update --check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Unknown,
    Checking,
    UpToDate,
    Available {
        latest: String,
        tag: String,
    },
    /// Offline or the release could not be resolved; not shown as an error.
    Failed,
}

/// The `/` action palette.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Palette {
    pub query: String,
    pub sel: usize,
}

/// One palette line: a screen to go to, or an action on its screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub screen: Screen,
    pub act: Option<Act>,
    pub label: String,
}

/// A selectable Home row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeItem {
    /// A failing or warning check, by index in `checks`.
    Check(usize),
    FixAll,
    Pair,
    Update,
    Wizard,
}

pub struct App {
    pub lang: Lang,
    pub screen: Screen,
    pub mode: Mode,
    pub message: Option<Message>,
    loaded: [bool; 8],
    sel: [usize; 8],
    pub version: Option<String>,
    pub healthy: Option<bool>,
    pub checks: Vec<CheckRow>,
    pub update: UpdateState,
    pub agents: Vec<AgentRow>,
    pub events: Vec<EventRow>,
    pub devices: Vec<DeviceRow>,
    pub integrations: Vec<IntegrationRow>,
    pub invert: bool,
    pub service_output: Option<String>,
    pub config_text: String,
    pub usage_days: i64,
    pub usage_text: String,
    pub uninstall_all: bool,
    pub plan: String,
    pub plan_empty: bool,
    /// Output of the last Advanced action (setup, uninstall); the plan otherwise.
    pub output: Option<String>,
    /// Set after a complete uninstall: nothing else may run (it would
    /// recreate state).
    pub uninstalled: bool,
    /// The open setup wizard's step.
    pub wizard: Option<usize>,
    /// The `?` overlay is open.
    pub help: bool,
    pub palette: Option<Palette>,
    integrations_known: bool,
}

/// Terminal-safe text: control characters (escape sequences) are replaced.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() && c != '\n' { '?' } else { c })
        .collect()
}

fn text(v: &Value, k: &str) -> String {
    clean(v[k].as_str().unwrap_or_default())
}

fn rows<T>(v: &Value, k: &str, f: impl Fn(&Value) -> T) -> Vec<T> {
    v[k].as_array()
        .map(|a| a.iter().map(f).collect())
        .unwrap_or_default()
}

fn json() -> JsonFlag {
    JsonFlag { json: true }
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self::with_lang(Lang::En)
    }

    pub fn with_lang(lang: Lang) -> Self {
        Self {
            lang,
            screen: Screen::Home,
            mode: Mode::Normal,
            message: None,
            loaded: [false; 8],
            sel: [0; 8],
            version: None,
            healthy: None,
            checks: Vec::new(),
            update: UpdateState::Unknown,
            agents: Vec::new(),
            events: Vec::new(),
            devices: Vec::new(),
            integrations: Vec::new(),
            invert: false,
            service_output: None,
            config_text: String::new(),
            usage_days: 7,
            usage_text: String::new(),
            uninstall_all: false,
            plan: String::new(),
            plan_empty: true,
            output: None,
            uninstalled: false,
            wizard: None,
            help: false,
            palette: None,
            integrations_known: false,
        }
    }

    pub fn tr(&self, en: &'static str) -> &'static str {
        t(self.lang, en)
    }

    /// Loads the first screen.
    pub fn start(&mut self, exec: &mut dyn Exec) {
        self.refresh(exec);
        if self.first_run() {
            self.open_wizard(exec);
        }
    }

    /// No phone paired yet and a prerequisite or the config is missing.
    pub fn first_run(&self) -> bool {
        const PREREQS: [FixId; 6] = [
            FixId::InstallRuntime,
            FixId::InstallTailscale,
            FixId::TailscaleUp,
            FixId::InstallSsh,
            FixId::EnableSsh,
            FixId::InstallService,
        ];
        let unpaired = self.checks.iter().any(|c| c.fix == Some(FixId::Pair));
        let missing = self.checks.iter().any(|c| {
            c.fix.is_some_and(|f| PREREQS.contains(&f)) || (c.name == "config" && !c.ok())
        });
        unpaired && missing
    }

    pub fn wizard_step(&self) -> Option<WizardStep> {
        self.wizard.map(|i| WizardStep::ALL[i])
    }

    /// The fix the step needs now, if any.
    pub fn step_fix(&self, step: WizardStep) -> Option<FixId> {
        self.checks
            .iter()
            .filter_map(|c| c.fix)
            .find(|f| step.fixes().contains(f))
    }

    fn undone_integrations(&self) -> Vec<(AgentArg, String)> {
        self.integrations
            .iter()
            .filter(|r| r.detected && r.installed != Some(true))
            .filter_map(|r| {
                <AgentArg as clap::ValueEnum>::from_str(&r.agent, true)
                    .ok()
                    .map(|a| (a, r.name.clone()))
            })
            .collect()
    }

    pub fn step_status(&self, step: WizardStep) -> StepStatus {
        match step {
            WizardStep::Integrations if !self.integrations_known => StepStatus::Unknown,
            WizardStep::Integrations if self.undone_integrations().is_empty() => StepStatus::Done,
            WizardStep::Integrations => StepStatus::Todo,
            _ if self.checks.is_empty() => StepStatus::Unknown,
            _ if self.step_fix(step).is_some() => StepStatus::Todo,
            _ => StepStatus::Done,
        }
    }

    /// Opens the wizard at its first step that is not done.
    pub fn open_wizard(&mut self, exec: &mut dyn Exec) {
        if let Err(e) = self.load_integrations(exec) {
            self.error(e.message);
        }
        self.wizard = Some(0);
        self.settle_wizard();
    }

    /// Moves past steps that are already done (never past the last one).
    fn settle_wizard(&mut self) {
        while let Some(i) = self.wizard {
            if i + 1 < WizardStep::ALL.len()
                && self.step_status(WizardStep::ALL[i]) == StepStatus::Done
            {
                self.wizard = Some(i + 1);
            } else {
                break;
            }
        }
    }

    fn next_step(&mut self) {
        match self.wizard {
            Some(i) if i + 1 < WizardStep::ALL.len() => {
                self.wizard = Some(i + 1);
                self.settle_wizard();
            }
            _ => {
                self.wizard = None;
                self.screen = Screen::Home;
                self.clamp();
                self.info(self.tr("Setup wizard finished. Reopen it from Home."));
            }
        }
    }

    /// Reads health and integrations again for the wizard.
    fn reload_wizard(&mut self, exec: &mut dyn Exec) {
        self.screen = Screen::Home;
        self.refresh(exec);
        if let Err(e) = self.load_integrations(exec) {
            self.error(e.message);
        }
    }

    fn on_wizard_key(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        let Some(step) = self.wizard_step() else {
            return Effect::None;
        };
        match key.code {
            KeyCode::Esc => {
                self.wizard = None;
                self.screen = Screen::Home;
                self.clamp();
            }
            KeyCode::Char('q') => return Effect::Quit,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('s') => self.next_step(),
            KeyCode::Char('r') => self.reload_wizard(exec),
            KeyCode::Enter => match self.step_status(step) {
                StepStatus::Done => self.next_step(),
                StepStatus::Unknown => self.reload_wizard(exec),
                StepStatus::Todo if step == WizardStep::Integrations => {
                    self.mode = Mode::Confirm(Pending::InstallDetected {
                        agents: self.undone_integrations(),
                    })
                }
                StepStatus::Todo => match self.step_fix(step) {
                    Some(FixId::Pair) => {
                        return Effect::Pair {
                            invert: self.invert,
                        }
                    }
                    Some(id) => self.mode = Mode::Confirm(Pending::Fix { id }),
                    None => self.next_step(),
                },
            },
            _ => {}
        }
        Effect::None
    }

    pub fn selected(&self) -> usize {
        self.sel[self.screen.index()]
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Home => self.home_items().len(),
            Screen::Agents => self.events.len(),
            Screen::Phones | Screen::Settings => self.devices.len(),
            Screen::Integrations => self.integrations.len(),
            Screen::Advanced => self.advanced_items().len(),
            Screen::Service | Screen::Usage => 0,
        }
    }

    /// Fixes `doctor --fix` would run, each once, in check order. Pairing is
    /// separate: it needs a person.
    pub fn pending_fixes(&self) -> Vec<FixId> {
        let mut out = Vec::new();
        for id in self.checks.iter().filter_map(|c| c.fix) {
            if id != FixId::Pair && !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    pub fn home_items(&self) -> Vec<HomeItem> {
        let mut v: Vec<HomeItem> = self
            .checks
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.ok())
            .map(|(i, _)| HomeItem::Check(i))
            .collect();
        if !self.pending_fixes().is_empty() {
            v.push(HomeItem::FixAll);
        }
        v.push(HomeItem::Pair);
        if matches!(self.update, UpdateState::Available { .. }) {
            v.push(HomeItem::Update);
        }
        v.push(HomeItem::Wizard);
        v
    }

    pub fn advanced_items(&self) -> Vec<Act> {
        let mut v = vec![Act::HardenSsh, Act::Setup, Act::SetupAll];
        if self.checks.iter().any(|c| c.name == "legacy" && !c.ok()) {
            v.push(Act::LegacyCleanup);
        }
        v.push(Act::Uninstall);
        v
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            error: false,
            text: clean(&text.into()),
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            error: true,
            text: clean(&text.into()),
        });
    }

    /// The background update check started.
    pub fn checking_updates(&mut self) {
        self.update = UpdateState::Checking;
    }

    /// Result of `update --check --json` (its data), or why it failed.
    pub fn on_update_check(&mut self, r: Result<Value, String>) {
        self.update = match r {
            Ok(v) if v["update_available"] == true => UpdateState::Available {
                latest: text(&v, "latest"),
                tag: text(&v, "tag"),
            },
            Ok(_) => UpdateState::UpToDate,
            Err(_) => UpdateState::Failed,
        };
        self.clamp();
    }

    pub fn on_key(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        if key.kind != KeyEventKind::Press {
            return Effect::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal if self.palette.is_some() => self.on_palette_key(key, exec),
            Mode::Normal if self.help => {
                if matches!(
                    key.code,
                    KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
                ) {
                    self.help = false;
                }
                Effect::None
            }
            Mode::Normal if self.wizard.is_some() => self.on_wizard_key(key, exec),
            Mode::Normal => self.on_normal(key, exec),
            Mode::Confirm(p) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.perform(p, exec),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.info(self.tr("Cancelled; nothing changed."));
                    Effect::None
                }
                _ => {
                    self.mode = Mode::Confirm(p);
                    Effect::None
                }
            },
            Mode::TypeConfirm { mut input } => {
                match key.code {
                    KeyCode::Esc => self.info(self.tr("Uninstall cancelled; nothing changed.")),
                    KeyCode::Enter if input == UNINSTALL_WORD => self.uninstall(exec),
                    KeyCode::Enter => self.error(tf(
                        self.lang,
                        "Type \"{}\" to confirm; nothing changed.",
                        &[UNINSTALL_WORD],
                    )),
                    KeyCode::Backspace => {
                        input.pop();
                        self.mode = Mode::TypeConfirm { input };
                    }
                    KeyCode::Char(c) if input.len() < 32 => {
                        input.push(c);
                        self.mode = Mode::TypeConfirm { input };
                    }
                    _ => self.mode = Mode::TypeConfirm { input },
                }
                Effect::None
            }
            Mode::TextInput { purpose, mut input } => {
                match key.code {
                    KeyCode::Esc => self.info(self.tr("Cancelled; nothing changed.")),
                    KeyCode::Enter if input.trim().is_empty() => {
                        self.error(self.tr("Empty input; nothing changed."))
                    }
                    KeyCode::Enter => {
                        let value = input.trim().to_string();
                        self.mode = Mode::Confirm(match purpose {
                            Input::Reply { event_id, title } => Pending::Respond {
                                event_id,
                                title,
                                action: ActionArg::Text,
                                text: Some(input),
                            },
                            Input::Webpush { id, name } => Pending::SetWebpush {
                                id,
                                name,
                                subscription: value,
                            },
                            Input::Endpoint { id, name } => Pending::SetEndpoint {
                                id,
                                name,
                                url: value,
                            },
                        });
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        self.mode = Mode::TextInput { purpose, input };
                    }
                    KeyCode::Char(c) => {
                        if input.len() + c.len_utf8() <= purpose.max() && !c.is_control() {
                            input.push(c);
                        }
                        self.mode = Mode::TextInput { purpose, input };
                    }
                    _ => self.mode = Mode::TextInput { purpose, input },
                }
                Effect::None
            }
        }
    }

    fn on_normal(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        let i = self.screen.index();
        let n = Screen::ALL.len();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Effect::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.goto(Screen::ALL[(i + 1) % n], exec)
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.goto(Screen::ALL[(i + n - 1) % n], exec)
            }
            KeyCode::Char(c @ '1'..='8') => {
                self.goto(Screen::ALL[c as usize - '1' as usize], exec);
            }
            KeyCode::Char('r') => self.refresh(exec),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.palette = Some(Palette::default()),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.sel[i] + 1 < self.list_len() {
                    self.sel[i] += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.sel[i] = self.sel[i].saturating_sub(1),
            code => {
                if let Some(act) = actions::for_key(self.screen, code) {
                    return self.run_act(act, exec);
                }
            }
        }
        Effect::None
    }

    /// Every screen and every action that can run now.
    pub fn palette_entries(&self) -> Vec<PaletteEntry> {
        let mut v: Vec<PaletteEntry> = Screen::ALL
            .iter()
            .map(|s| PaletteEntry {
                screen: *s,
                act: None,
                label: tf(self.lang, "Go to {}", &[self.tr(s.title())]),
            })
            .collect();
        let advanced = self.advanced_items();
        for d in actions::ACTIONS {
            let hidden = match d.act {
                Act::Activate => true,
                Act::Update => !matches!(self.update, UpdateState::Available { .. }),
                Act::LegacyCleanup => !advanced.contains(&Act::LegacyCleanup),
                _ => false,
            };
            if !hidden {
                v.push(PaletteEntry {
                    screen: d.screen,
                    act: Some(d.act),
                    label: self.tr(d.label).to_string(),
                });
            }
        }
        v
    }

    /// Palette entries whose label or screen contains every word typed.
    pub fn palette_matches(&self) -> Vec<PaletteEntry> {
        let query = self
            .palette
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        let words: Vec<&str> = query.split_whitespace().collect();
        self.palette_entries()
            .into_iter()
            .filter(|e| {
                let hay = format!("{} {}", e.label, self.tr(e.screen.tab())).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }

    fn on_palette_key(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        let matches = self.palette_matches();
        let n = matches.len();
        let Some(p) = self.palette.as_mut() else {
            return Effect::None;
        };
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Enter => match matches.get(p.sel).cloned() {
                Some(e) => {
                    self.palette = None;
                    self.goto(e.screen, exec);
                    if let Some(act) = e.act {
                        return self.run_act(act, exec);
                    }
                }
                None => self.error(self.tr("No matching action.")),
            },
            KeyCode::Down | KeyCode::Tab => {
                if p.sel + 1 < n {
                    p.sel += 1;
                }
            }
            KeyCode::Up | KeyCode::BackTab => p.sel = p.sel.saturating_sub(1),
            KeyCode::Backspace => {
                p.query.pop();
                p.sel = 0;
            }
            KeyCode::Char(c) if !c.is_control() && p.query.len() < 64 => {
                p.query.push(c);
                p.sel = 0;
            }
            _ => {}
        }
        Effect::None
    }

    /// Runs `act` on the current screen and selection.
    pub fn run_act(&mut self, act: Act, exec: &mut dyn Exec) -> Effect {
        if self.uninstalled {
            self.error(self.tr("falconer-hook was removed; press q to quit."));
            return Effect::None;
        }
        let sel = self.selected();
        match act {
            Act::Activate => return self.activate(exec),
            Act::Fix => match self.home_items().get(sel) {
                Some(HomeItem::Check(i)) => return self.fix_check(*i),
                _ => self.error(self.tr("Select a problem to fix.")),
            },
            Act::FixAll => {
                let ids = self.pending_fixes();
                if ids.is_empty() {
                    self.info(self.tr("Nothing to fix."));
                } else {
                    self.mode = Mode::Confirm(Pending::FixAll { ids });
                }
            }
            Act::Wizard => self.open_wizard(exec),
            Act::Update => match &self.update {
                UpdateState::Available { tag, .. } => {
                    self.mode = Mode::Confirm(Pending::Update { tag: tag.clone() })
                }
                UpdateState::Checking => self.info(self.tr("Still checking for updates.")),
                _ => self.info(self.tr("No update available.")),
            },
            Act::Allow | Act::Deny | Act::Reply => {
                let Some(e) = self.events.get(sel).cloned() else {
                    self.error(self.tr("No event selected."));
                    return Effect::None;
                };
                if !e.pending {
                    self.error(self.tr("That event is already resolved."));
                    return Effect::None;
                }
                self.mode = match act {
                    Act::Reply => Mode::TextInput {
                        purpose: Input::Reply {
                            event_id: e.id,
                            title: e.title,
                        },
                        input: String::new(),
                    },
                    _ => Mode::Confirm(Pending::Respond {
                        event_id: e.id,
                        title: e.title,
                        action: if act == Act::Allow {
                            ActionArg::Allow
                        } else {
                            ActionArg::Deny
                        },
                        text: None,
                    }),
                };
            }
            Act::Pair => {
                return Effect::Pair {
                    invert: self.invert,
                }
            }
            Act::InvertQr => self.invert = !self.invert,
            Act::Revoke | Act::SetWebpush | Act::ClearWebpush | Act::SetEndpoint => {
                let Some(d) = self.devices.get(sel).cloned() else {
                    self.error(self.tr("No phone selected."));
                    return Effect::None;
                };
                let (id, name) = (d.id, d.name);
                self.mode = match act {
                    Act::Revoke => Mode::Confirm(Pending::Revoke { id, name }),
                    Act::ClearWebpush => Mode::Confirm(Pending::ClearWebpush { id, name }),
                    Act::SetWebpush => Mode::TextInput {
                        purpose: Input::Webpush { id, name },
                        input: String::new(),
                    },
                    _ => Mode::TextInput {
                        purpose: Input::Endpoint { id, name },
                        input: String::new(),
                    },
                };
            }
            Act::IntegrationInstall | Act::IntegrationUninstall => {
                let row = self.integrations.get(sel).cloned();
                match row.and_then(|r| {
                    <AgentArg as clap::ValueEnum>::from_str(&r.agent, true)
                        .ok()
                        .map(|a| (a, r))
                }) {
                    Some((agent, r)) => {
                        self.mode = Mode::Confirm(Pending::Integration {
                            agent,
                            name: r.name,
                            install: act == Act::IntegrationInstall,
                        })
                    }
                    None => self.error(self.tr("No agent selected.")),
                }
            }
            Act::ServiceInstall => self.mode = Mode::Confirm(Pending::Service(ServiceOp::Install)),
            Act::ServiceRestart => self.mode = Mode::Confirm(Pending::Service(ServiceOp::Restart)),
            Act::ServiceUninstall => {
                self.mode = Mode::Confirm(Pending::Service(ServiceOp::Uninstall))
            }
            Act::MoreDays | Act::FewerDays => {
                let i = USAGE_DAYS
                    .iter()
                    .position(|d| *d == self.usage_days)
                    .unwrap_or(1);
                let j = if act == Act::MoreDays {
                    (i + 1).min(USAGE_DAYS.len() - 1)
                } else {
                    i.saturating_sub(1)
                };
                if j != i {
                    self.usage_days = USAGE_DAYS[j];
                    self.refresh(exec);
                }
            }
            Act::HardenSsh => return Effect::HardenSsh,
            Act::Setup => self.mode = Mode::Confirm(Pending::Setup { yes: false }),
            Act::SetupAll => self.mode = Mode::Confirm(Pending::Setup { yes: true }),
            Act::LegacyCleanup => self.mode = Mode::Confirm(Pending::LegacyCleanup),
            Act::ToggleAll => {
                self.uninstall_all = !self.uninstall_all;
                self.output = None;
                self.refresh(exec);
            }
            Act::Uninstall => {
                if self.plan_empty {
                    self.info(self.tr("Nothing to remove."));
                } else {
                    self.output = None;
                    self.mode = Mode::TypeConfirm {
                        input: String::new(),
                    };
                }
            }
        }
        Effect::None
    }

    /// Enter: the selected row's action.
    fn activate(&mut self, exec: &mut dyn Exec) -> Effect {
        let sel = self.selected();
        match self.screen {
            Screen::Home => match self.home_items().get(sel).copied() {
                Some(HomeItem::Check(i)) => self.fix_check(i),
                Some(HomeItem::FixAll) => self.run_act(Act::FixAll, exec),
                Some(HomeItem::Pair) => self.run_act(Act::Pair, exec),
                Some(HomeItem::Update) => self.run_act(Act::Update, exec),
                Some(HomeItem::Wizard) => self.run_act(Act::Wizard, exec),
                None => Effect::None,
            },
            Screen::Advanced => match self.advanced_items().get(sel).copied() {
                Some(act) => self.run_act(act, exec),
                None => Effect::None,
            },
            _ => Effect::None,
        }
    }

    fn fix_check(&mut self, i: usize) -> Effect {
        let Some(c) = self.checks.get(i).cloned() else {
            return Effect::None;
        };
        match c.fix {
            None if c.ok() => self.info(tf(self.lang, "{} is OK.", &[&c.name])),
            None => self.info(tf(
                self.lang,
                "No automatic fix for {}: {}",
                &[&c.name, &c.detail],
            )),
            Some(FixId::Pair) => {
                return Effect::Pair {
                    invert: self.invert,
                }
            }
            Some(id) => self.mode = Mode::Confirm(Pending::Fix { id }),
        }
        Effect::None
    }

    pub fn goto(&mut self, screen: Screen, exec: &mut dyn Exec) {
        self.screen = screen;
        if !self.loaded[screen.index()] {
            self.refresh(exec);
        }
    }

    fn load_doctor(&mut self, exec: &mut dyn Exec) -> Result<(), CliError> {
        let d = exec
            .exec(Command::Doctor {
                fix: None,
                yes: false,
                out: json(),
            })?
            .data;
        self.set_checks(&d);
        Ok(())
    }

    fn set_checks(&mut self, d: &Value) {
        self.healthy = d["healthy"].as_bool();
        self.checks = rows(d, "checks", |c| CheckRow {
            name: text(c, "name"),
            status: text(c, "status"),
            detail: text(c, "detail"),
            fix: c["fix"]["id"].as_str().and_then(FixId::parse),
        });
    }

    fn load_devices(&mut self, exec: &mut dyn Exec) -> Result<(), CliError> {
        let d = exec
            .exec(Command::Devices {
                cmd: DevicesCmd::List { out: json() },
            })?
            .data;
        self.devices = rows(&d, "devices", |d| DeviceRow {
            id: text(d, "id"),
            name: text(d, "name"),
            stale: d["endpoint_stale"].as_bool().unwrap_or(false),
            ssh: d["ssh_authorized"].as_bool().unwrap_or(false),
            no_push: d["endpoint"].as_str() == Some("") && d["webpush"] != true,
            endpoint: text(d, "endpoint"),
            webpush: d["webpush"].as_bool().unwrap_or(false),
        });
        Ok(())
    }

    fn load_integrations(&mut self, exec: &mut dyn Exec) -> Result<(), CliError> {
        let d = exec
            .exec(Command::Integration {
                cmd: IntegrationCmd::List { out: json() },
            })?
            .data;
        self.integrations = rows(&d, "agents", |a| IntegrationRow {
            agent: text(a, "agent"),
            name: text(a, "name"),
            detected: a["detected"].as_bool().unwrap_or(false),
            installed: a["installed"].as_bool(),
            settings: text(a, "settings"),
        });
        self.integrations_known = true;
        Ok(())
    }

    /// Reloads the current screen.
    pub fn refresh(&mut self, exec: &mut dyn Exec) {
        if self.uninstalled {
            return;
        }
        let i = self.screen.index();
        self.loaded[i] = true;
        let r: Result<(), CliError> = (|| {
            match self.screen {
                Screen::Home => {
                    self.load_doctor(exec)?;
                    let v = exec.exec(Command::Version { out: json() })?.data;
                    self.version = Some(text(&v, "version"));
                }
                Screen::Agents => {
                    let a = exec
                        .exec(Command::Agents {
                            cmd: ListCmd::List { out: json() },
                        })?
                        .data;
                    self.agents = rows(&a, "agents", |a| AgentRow {
                        pane: text(a, "pane_id"),
                        agent: text(a, "agent"),
                        status: text(a, "status"),
                        title: text(a, "title"),
                    });
                    let e = exec
                        .exec(Command::Inbox {
                            cmd: InboxCmd::List {
                                since: None,
                                limit: Some(100),
                                out: json(),
                            },
                        })?
                        .data;
                    let mut events = rows(&e, "events", |e| EventRow {
                        id: text(e, "id"),
                        seq: e["seq"].as_u64().unwrap_or_default(),
                        agent: text(e, "agent"),
                        kind: text(e, "kind"),
                        pending: e["status"] == "pending",
                        title: text(e, "title"),
                    });
                    // Newest first.
                    events.sort_by_key(|e| std::cmp::Reverse(e.seq));
                    self.events = events;
                }
                Screen::Phones => self.load_devices(exec)?,
                Screen::Settings => {
                    self.load_devices(exec)?;
                    let c = exec.exec(Command::Config { out: json() })?;
                    self.config_text = clean(&c.human);
                }
                Screen::Integrations => self.load_integrations(exec)?,
                Screen::Service => self.load_doctor(exec)?,
                Screen::Usage => {
                    let r = exec.exec(Command::Usage {
                        days: self.usage_days,
                        out: json(),
                    })?;
                    self.usage_text = clean(&r.human);
                }
                Screen::Advanced => {
                    let r = exec.exec(Command::Uninstall {
                        all: self.uninstall_all,
                        dry_run: true,
                        yes: false,
                        out: json(),
                    })?;
                    self.plan_empty = r.data["steps"].as_array().is_none_or(|s| s.is_empty());
                    self.plan = clean(&r.human);
                }
            }
            Ok(())
        })();
        self.clamp();
        if let Err(e) = r {
            self.error(e.message);
        }
    }

    /// Keeps the selection inside the current list.
    fn clamp(&mut self) {
        let i = self.screen.index();
        let n = self.list_len();
        if self.sel[i] >= n {
            self.sel[i] = n.saturating_sub(1);
        }
    }

    /// Marks every other screen stale after a command that changes state.
    fn invalidate(&mut self) {
        let keep = self.screen.index();
        for (i, l) in self.loaded.iter_mut().enumerate() {
            if i != keep {
                *l = false;
            }
        }
    }

    fn perform(&mut self, p: Pending, exec: &mut dyn Exec) -> Effect {
        if self.uninstalled {
            return Effect::None;
        }
        if let Pending::InstallDetected { agents } = &p {
            self.install_detected(agents, exec);
            return Effect::None;
        }
        let cmd = match &p {
            Pending::InstallDetected { .. } => return Effect::None,
            Pending::Fix { id } if *id != FixId::FixPermissions => {
                return Effect::Fix { id: *id };
            }
            Pending::FixAll { .. } => return Effect::FixAll,
            Pending::Update { tag } => return Effect::Update { tag: tag.clone() },
            Pending::Setup { yes: true } => return Effect::SetupAll,
            Pending::Fix { id } => Command::Doctor {
                fix: Some(Some(id.as_str().into())),
                yes: true,
                out: json(),
            },
            Pending::Respond {
                event_id,
                action,
                text,
                ..
            } => Command::Respond {
                event_id: event_id.clone(),
                action: *action,
                text: text.clone(),
                out: json(),
            },
            Pending::Revoke { id, .. } => Command::Devices {
                cmd: DevicesCmd::Revoke {
                    id: id.clone(),
                    out: json(),
                },
            },
            Pending::Integration { agent, install, .. } => Command::Integration {
                cmd: if *install {
                    IntegrationCmd::Install {
                        agent: *agent,
                        settings: None,
                        out: json(),
                    }
                } else {
                    IntegrationCmd::Uninstall {
                        agent: *agent,
                        settings: None,
                        out: json(),
                    }
                },
            },
            Pending::Setup { yes: false } | Pending::LegacyCleanup => Command::Setup {
                yes: false,
                no_service: false,
                out: json(),
            },
            Pending::Service(op) => Command::Service {
                cmd: match op {
                    ServiceOp::Install => ServiceCmd::Install {
                        dry_run: false,
                        out: json(),
                    },
                    ServiceOp::Restart => ServiceCmd::Restart { out: json() },
                    ServiceOp::Uninstall => ServiceCmd::Uninstall {
                        dry_run: false,
                        out: json(),
                    },
                },
            },
            Pending::SetEndpoint { id, url, .. } => Command::Devices {
                cmd: DevicesCmd::SetEndpoint {
                    id: id.clone(),
                    url: url.clone(),
                    out: json(),
                },
            },
            Pending::SetWebpush { id, .. } => Command::Devices {
                cmd: DevicesCmd::SetWebpush {
                    id: id.clone(),
                    out: json(),
                },
            },
            Pending::ClearWebpush { id, .. } => Command::Devices {
                cmd: DevicesCmd::ClearWebpush {
                    id: id.clone(),
                    out: json(),
                },
            },
        };
        let r = match &p {
            Pending::SetWebpush { subscription, .. } => exec.exec_stdin(cmd, subscription),
            _ => exec.exec(cmd),
        };
        match r {
            Ok(reply) => {
                let human = match &p {
                    Pending::Respond { action, .. } => tf(
                        self.lang,
                        match action {
                            ActionArg::Allow => "Sent allow.",
                            ActionArg::Deny => "Sent deny.",
                            ActionArg::Text => "Sent the reply.",
                        },
                        &[],
                    ),
                    _ => match first_line(&reply.human) {
                        "" => self.tr("Done.").to_string(),
                        l => l.to_string(),
                    },
                };
                match &p {
                    Pending::Setup { .. } | Pending::LegacyCleanup => {
                        self.output = Some(clean(&reply.human))
                    }
                    Pending::Service(_) => self.service_output = Some(clean(&reply.human)),
                    Pending::Fix { .. } => self.set_checks(&reply.data),
                    _ => {}
                }
                self.info(human);
                self.invalidate();
                let msg = self.message.clone();
                self.refresh(exec);
                self.settle_wizard();
                // Keep the action's outcome unless the refresh failed.
                if !self.message.as_ref().is_some_and(|m| m.error) {
                    self.message = msg;
                }
            }
            Err(e) => self.error(e.message),
        }
        Effect::None
    }

    fn install_detected(&mut self, agents: &[(AgentArg, String)], exec: &mut dyn Exec) {
        let mut failed = Vec::new();
        for (agent, name) in agents {
            let r = exec.exec(Command::Integration {
                cmd: IntegrationCmd::Install {
                    agent: *agent,
                    settings: None,
                    out: json(),
                },
            });
            if let Err(e) = r {
                failed.push(format!("{name}: {}", e.message));
            }
        }
        self.invalidate();
        let _ = self.load_integrations(exec);
        if failed.is_empty() {
            self.info(tf(
                self.lang,
                "Installed {} integrations.",
                &[&agents.len().to_string()],
            ));
            self.settle_wizard();
        } else {
            self.error(failed.join("; "));
        }
    }

    fn uninstall(&mut self, exec: &mut dyn Exec) {
        if self.uninstalled {
            return;
        }
        match exec.exec(Command::Uninstall {
            all: self.uninstall_all,
            dry_run: false,
            yes: true,
            out: json(),
        }) {
            Ok(r) => {
                self.output = Some(clean(&r.human));
                if r.data["complete"].as_bool().unwrap_or(false) {
                    self.uninstalled = true;
                    self.plan_empty = true;
                    self.info(self.tr("falconer-hook was removed. Press q to quit."));
                } else {
                    self.error(self.tr("Some steps failed; see the list, then try again."));
                    self.invalidate();
                }
            }
            Err(e) => self.error(e.message),
        }
    }

    /// Result of a command run while the TUI was suspended.
    pub fn after_suspended(&mut self, r: Result<Reply, CliError>) {
        match r {
            Ok(reply) => self.info(first_line(&reply.human).to_string()),
            Err(e) => self.error(e.message),
        }
        self.invalidate();
    }

    /// Records the outcome of the pairing flow run outside the TUI.
    pub fn after_pair(&mut self, r: Result<Reply, CliError>) {
        self.after_suspended(r)
    }

    /// Back from the terminal: reload what the command may have changed,
    /// keeping its outcome message.
    pub fn resume(&mut self, exec: &mut dyn Exec) {
        let msg = self.message.clone();
        self.loaded = [false; 8];
        self.refresh(exec);
        if self.wizard.is_some() {
            let _ = self.load_integrations(exec);
        }
        if self.screen != Screen::Home && !self.uninstalled {
            // Home's health drives the problems list and the wizard.
            let screen = self.screen;
            self.screen = Screen::Home;
            self.refresh(exec);
            self.screen = screen;
        }
        self.settle_wizard();
        if !self.message.as_ref().is_some_and(|m| m.error) {
            self.message = msg;
        }
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hub_tests.rs"]
mod hub_tests;

#[cfg(test)]
#[path = "wizard_tests.rs"]
mod wizard_tests;

#[cfg(test)]
#[path = "palette_tests.rs"]
mod palette_tests;
