//! TUI state machine: screens, selection and confirmation gating.
//!
//! Every action is a `cli::Command` run through the `Exec` port, so the TUI
//! uses exactly the same use cases as the CLI and is testable without a
//! terminal.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

use crate::cli::output::{CliError, Reply};
use crate::cli::{
    ActionArg, AgentArg, Command, DevicesCmd, InboxCmd, IntegrationCmd, JsonFlag, ListCmd,
};

/// Runs one CLI command (the composition root in production, a fake in tests).
pub trait Exec {
    fn exec(&mut self, cmd: Command) -> Result<Reply, CliError>;
}

/// Word the user must type to confirm `uninstall`.
pub const UNINSTALL_WORD: &str = "uninstall";
const MAX_TEXT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Status,
    Agents,
    Devices,
    Pair,
    Integrations,
    Setup,
    Uninstall,
}

impl Screen {
    pub const ALL: [Screen; 7] = [
        Screen::Status,
        Screen::Agents,
        Screen::Devices,
        Screen::Pair,
        Screen::Integrations,
        Screen::Setup,
        Screen::Uninstall,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Status => "Status",
            Screen::Agents => "Agents & inbox",
            Screen::Devices => "Devices",
            Screen::Pair => "Pair",
            Screen::Integrations => "Integrations",
            Screen::Setup => "Setup",
            Screen::Uninstall => "Uninstall",
        }
    }

    pub fn index(self) -> usize {
        Screen::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Key hints shown in the footer, besides the global ones.
    pub fn keys(self) -> &'static str {
        match self {
            Screen::Status => "r refresh",
            Screen::Agents => "j/k select  a allow  d deny  t reply text  r refresh",
            Screen::Devices => "j/k select  x revoke  r refresh",
            Screen::Pair => "Enter pair  i invert QR colors",
            Screen::Integrations => "j/k select  i install  u uninstall  r refresh",
            Screen::Setup => "s setup  a setup + install every detected integration",
            Screen::Uninstall => "a toggle --all  Enter uninstall  r refresh plan",
        }
    }
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
}

impl Pending {
    pub fn question(&self) -> String {
        match self {
            Pending::Respond {
                title,
                action,
                text,
                ..
            } => match action {
                ActionArg::Allow => format!("Allow \"{title}\"?"),
                ActionArg::Deny => format!("Deny \"{title}\"?"),
                ActionArg::Text => format!(
                    "Send \"{}\" to \"{title}\"?",
                    text.as_deref().unwrap_or_default()
                ),
            },
            Pending::Revoke { name, .. } => {
                format!("Revoke {name}? Its SSH key and push registration are removed.")
            }
            Pending::Integration { name, install, .. } => format!(
                "{} the {name} integration?",
                if *install { "Install" } else { "Uninstall" }
            ),
            Pending::Setup { yes: false } => {
                "Run setup (config, user service, doctor)? Integrations are left as they are."
                    .into()
            }
            Pending::Setup { yes: true } => {
                "Run setup and install the integration of every detected agent?".into()
            }
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
    /// Free-text reply to an event, confirmed afterwards.
    TextInput {
        event_id: String,
        title: String,
        input: String,
    },
}

/// What the terminal loop must do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    /// Suspend the TUI and run the QR pairing flow in the terminal.
    Pair {
        invert: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    pub name: String,
    pub status: String,
    pub detail: String,
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

pub struct App {
    pub screen: Screen,
    pub mode: Mode,
    pub message: Option<Message>,
    loaded: [bool; 7],
    sel: [usize; 7],
    pub version: Option<String>,
    pub healthy: Option<bool>,
    pub checks: Vec<CheckRow>,
    pub agents: Vec<AgentRow>,
    pub events: Vec<EventRow>,
    pub devices: Vec<DeviceRow>,
    pub integrations: Vec<IntegrationRow>,
    pub invert: bool,
    pub setup_output: Option<String>,
    pub uninstall_all: bool,
    pub plan: String,
    pub plan_empty: bool,
    pub uninstall_output: Option<String>,
    /// Set after a complete uninstall: nothing else may run (it would
    /// recreate state).
    pub uninstalled: bool,
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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            screen: Screen::Status,
            mode: Mode::Normal,
            message: None,
            loaded: [false; 7],
            sel: [0; 7],
            version: None,
            healthy: None,
            checks: Vec::new(),
            agents: Vec::new(),
            events: Vec::new(),
            devices: Vec::new(),
            integrations: Vec::new(),
            invert: false,
            setup_output: None,
            uninstall_all: false,
            plan: String::new(),
            plan_empty: true,
            uninstall_output: None,
            uninstalled: false,
        }
    }

    /// Loads the first screen.
    pub fn start(&mut self, exec: &mut dyn Exec) {
        self.refresh(exec);
    }

    pub fn selected(&self) -> usize {
        self.sel[self.screen.index()]
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Agents => self.events.len(),
            Screen::Devices => self.devices.len(),
            Screen::Integrations => self.integrations.len(),
            _ => 0,
        }
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

    pub fn on_key(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        if key.kind != KeyEventKind::Press {
            return Effect::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.on_normal(key, exec),
            Mode::Confirm(p) => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => self.perform(p, exec),
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        self.info("Cancelled; nothing changed.")
                    }
                    _ => self.mode = Mode::Confirm(p),
                }
                Effect::None
            }
            Mode::TypeConfirm { mut input } => {
                match key.code {
                    KeyCode::Esc => self.info("Uninstall cancelled; nothing changed."),
                    KeyCode::Enter if input == UNINSTALL_WORD => self.uninstall(exec),
                    KeyCode::Enter => {
                        self.error(format!(
                            "Type \"{UNINSTALL_WORD}\" to confirm; nothing changed."
                        ));
                    }
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
            Mode::TextInput {
                event_id,
                title,
                mut input,
            } => {
                match key.code {
                    KeyCode::Esc => self.info("Cancelled; nothing sent."),
                    KeyCode::Enter if input.trim().is_empty() => {
                        self.error("Empty reply; nothing sent.")
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Confirm(Pending::Respond {
                            event_id,
                            title,
                            action: ActionArg::Text,
                            text: Some(input),
                        })
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        self.mode = Mode::TextInput {
                            event_id,
                            title,
                            input,
                        };
                    }
                    KeyCode::Char(c) => {
                        if input.len() + c.len_utf8() <= MAX_TEXT && !c.is_control() {
                            input.push(c);
                        }
                        self.mode = Mode::TextInput {
                            event_id,
                            title,
                            input,
                        };
                    }
                    _ => {
                        self.mode = Mode::TextInput {
                            event_id,
                            title,
                            input,
                        }
                    }
                }
                Effect::None
            }
        }
    }

    fn on_normal(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        let i = self.screen.index();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Effect::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.goto(Screen::ALL[(i + 1) % Screen::ALL.len()], exec)
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.goto(
                    Screen::ALL[(i + Screen::ALL.len() - 1) % Screen::ALL.len()],
                    exec,
                );
            }
            KeyCode::Char(c @ '1'..='7') => {
                self.goto(Screen::ALL[c as usize - '1' as usize], exec);
            }
            KeyCode::Char('r') => self.refresh(exec),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.sel[i] + 1 < self.list_len() {
                    self.sel[i] += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.sel[i] = self.sel[i].saturating_sub(1),
            _ => return self.on_screen_key(key, exec),
        }
        Effect::None
    }

    fn on_screen_key(&mut self, key: KeyEvent, exec: &mut dyn Exec) -> Effect {
        if self.uninstalled {
            self.error("openmosh-hook was removed; press q to quit.");
            return Effect::None;
        }
        let sel = self.selected();
        match (self.screen, key.code) {
            (Screen::Agents, KeyCode::Char(c @ ('a' | 'd' | 't'))) => {
                let Some(e) = self.events.get(sel).cloned() else {
                    self.error("No event selected.");
                    return Effect::None;
                };
                if !e.pending {
                    self.error("That event is already resolved.");
                    return Effect::None;
                }
                self.mode = match c {
                    't' => Mode::TextInput {
                        event_id: e.id,
                        title: e.title,
                        input: String::new(),
                    },
                    _ => Mode::Confirm(Pending::Respond {
                        event_id: e.id,
                        title: e.title,
                        action: if c == 'a' {
                            ActionArg::Allow
                        } else {
                            ActionArg::Deny
                        },
                        text: None,
                    }),
                };
            }
            (Screen::Devices, KeyCode::Char('x') | KeyCode::Delete) => {
                match self.devices.get(sel).cloned() {
                    Some(d) => {
                        self.mode = Mode::Confirm(Pending::Revoke {
                            id: d.id,
                            name: d.name,
                        })
                    }
                    None => self.error("No device selected."),
                }
            }
            (Screen::Pair, KeyCode::Enter | KeyCode::Char('p')) => {
                return Effect::Pair {
                    invert: self.invert,
                }
            }
            (Screen::Pair, KeyCode::Char('i')) => self.invert = !self.invert,
            (Screen::Integrations, KeyCode::Char(c @ ('i' | 'u'))) => {
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
                            install: c == 'i',
                        })
                    }
                    None => self.error("No agent selected."),
                }
            }
            (Screen::Setup, KeyCode::Char(c @ ('s' | 'a'))) => {
                self.mode = Mode::Confirm(Pending::Setup { yes: c == 'a' })
            }
            (Screen::Uninstall, KeyCode::Char('a')) => {
                self.uninstall_all = !self.uninstall_all;
                self.refresh(exec);
            }
            (Screen::Uninstall, KeyCode::Enter | KeyCode::Char('u')) => {
                if self.plan_empty {
                    self.info("Nothing to remove.");
                } else {
                    self.mode = Mode::TypeConfirm {
                        input: String::new(),
                    };
                }
            }
            _ => {}
        }
        Effect::None
    }

    pub fn goto(&mut self, screen: Screen, exec: &mut dyn Exec) {
        self.screen = screen;
        if !self.loaded[screen.index()] {
            self.refresh(exec);
        }
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
                Screen::Status => {
                    let d = exec.exec(Command::Doctor { out: json() })?.data;
                    self.healthy = d["healthy"].as_bool();
                    self.checks = rows(&d, "checks", |c| CheckRow {
                        name: text(c, "name"),
                        status: text(c, "status"),
                        detail: text(c, "detail"),
                    });
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
                Screen::Devices => {
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
                    });
                }
                Screen::Integrations => {
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
                }
                Screen::Uninstall => {
                    let r = exec.exec(Command::Uninstall {
                        all: self.uninstall_all,
                        dry_run: true,
                        yes: false,
                        out: json(),
                    })?;
                    self.plan_empty = r.data["steps"].as_array().is_none_or(|s| s.is_empty());
                    self.plan = clean(&r.human);
                }
                Screen::Pair | Screen::Setup => {}
            }
            Ok(())
        })();
        let n = self.list_len();
        if self.sel[i] >= n {
            self.sel[i] = n.saturating_sub(1);
        }
        if let Err(e) = r {
            self.error(e.message);
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

    fn perform(&mut self, p: Pending, exec: &mut dyn Exec) {
        if self.uninstalled {
            return;
        }
        let cmd = match &p {
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
            Pending::Setup { yes } => Command::Setup {
                yes: *yes,
                no_service: false,
                out: json(),
            },
        };
        match exec.exec(cmd) {
            Ok(reply) => {
                let human = match &p {
                    Pending::Respond { action, .. } => format!("Sent {}.", action_name(*action)),
                    _ => reply.human.lines().next().unwrap_or("Done.").to_string(),
                };
                if let Pending::Setup { .. } = p {
                    self.setup_output = Some(clean(&reply.human));
                }
                self.info(human);
                self.invalidate();
                let msg = self.message.clone();
                if self.screen != Screen::Setup {
                    self.refresh(exec);
                }
                // Keep the action's outcome unless the refresh failed.
                if !self.message.as_ref().is_some_and(|m| m.error) {
                    self.message = msg;
                }
            }
            Err(e) => self.error(e.message),
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
                self.uninstall_output = Some(clean(&r.human));
                if r.data["complete"].as_bool().unwrap_or(false) {
                    self.uninstalled = true;
                    self.plan_empty = true;
                    self.info("openmosh-hook was removed. Press q to quit.");
                } else {
                    self.error("Some steps failed; see the list, then try again.");
                    self.invalidate();
                }
            }
            Err(e) => self.error(e.message),
        }
    }

    /// Records the outcome of the pairing flow run outside the TUI.
    pub fn after_pair(&mut self, r: Result<Reply, CliError>) {
        match r {
            Ok(reply) => self.info(reply.human),
            Err(e) => self.error(e.message),
        }
        self.invalidate();
    }
}

fn action_name(a: ActionArg) -> &'static str {
    match a {
        ActionArg::Allow => "allow",
        ActionArg::Deny => "deny",
        ActionArg::Text => "the reply",
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
