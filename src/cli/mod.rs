//! Command-line surface (clap) and output formatting. No adapters here.

pub mod output;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "openmosh-hook",
    version,
    about = "OpenMosh host daemon and CLI",
    propagate_version = true
)]
pub struct Cli {
    /// Without a command, opens the TUI on a terminal and prints this help otherwise.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, Args)]
pub struct JsonFlag {
    /// Print the result as a JSON envelope.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon: watch agents, store events, publish pushes.
    Serve,
    /// Record an event from an agent hook (event JSON on stdin).
    Emit {
        /// Agent name, e.g. `claude`.
        #[arg(long)]
        source: String,
        /// Event kind, for agents whose payload does not say it. The payload
        /// is then optional and only its message is kept.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Pair a phone by QR over a one-shot Tailscale-only listener.
    Pair {
        /// Host name shown on the phone (default: config host_alias or hostname).
        #[arg(long)]
        alias: Option<String>,
        /// Seconds to wait, at most 300.
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(10..=300))]
        timeout: u64,
        /// Render the QR dark-on-light (for light terminal themes).
        #[arg(long)]
        invert: bool,
        /// Also print the pairing URI. It carries the one-time token: treat it like the QR.
        #[arg(long)]
        uri: bool,
        /// Authorize the phone without asking `Pair "<name>" from <node> (<ip>)? [y/N]`.
        /// Needed when stdin is not a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Pair over an existing SSH session (request JSON on stdin).
    PairSsh {
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Herdr/tmux sessions.
    Sessions {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Agents with their state.
    Agents {
        #[command(subcommand)]
        cmd: ListCmd,
    },
    /// Inbox events.
    Inbox {
        #[command(subcommand)]
        cmd: InboxCmd,
    },
    /// Answer an event: deliver keys or text into the agent's pane.
    Respond {
        /// Event id (ids are b64u and may start with `-`).
        #[arg(allow_hyphen_values = true)]
        event_id: String,
        #[arg(long, value_enum)]
        action: ActionArg,
        /// Text for `--action text` (≤ 4 KiB, control characters removed).
        #[arg(long, allow_hyphen_values = true)]
        text: Option<String>,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Paired devices.
    Devices {
        #[command(subcommand)]
        cmd: DevicesCmd,
    },
    /// Add or remove agent hooks that call `emit`.
    Integration {
        #[command(subcommand)]
        cmd: IntegrationCmd,
    },
    /// First-time setup: config, user service, agent integrations, doctor.
    Setup {
        /// Install the integration of every detected agent without asking.
        #[arg(long)]
        yes: bool,
        /// Do not register the user service.
        #[arg(long)]
        no_service: bool,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Remove everything openmosh-hook created: service, integrations,
    /// paired keys, devices, state and config.
    Uninstall {
        /// Also delete the openmosh-hook binary that is running.
        #[arg(long)]
        all: bool,
        /// Print the plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Make sshd accept SSH keys only (no passwords), with sudo.
    HardenSsh {
        /// Print the plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Check Tailscale, Herdr/tmux, permissions, devices and the service.
    Doctor {
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Token usage per agent, project and model, read from the agents' own
    /// local logs (Codex only: the head and tail of its newest 200 session files;
    /// a session counts on the day of its last event). Claude Code is not read:
    /// it keeps no usage summary. Never calls a provider or reads credentials.
    Usage {
        /// Calendar days to cover, today included (1..=31).
        #[arg(long, default_value_t = 7, allow_negative_numbers = true)]
        days: i64,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Register or remove the user service.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Herdr navigator for the phone: structured snapshot, pane screen and a
    /// fixed allowlist of actions on a Herdr session's socket.
    Herdr {
        #[command(subcommand)]
        cmd: HerdrCmd,
    },
    /// Run a multiplexer client with focus mirroring across attached clients.
    Attach {
        #[command(subcommand)]
        cmd: AttachCmd,
    },
    /// Open the interactive terminal UI (also the default on a terminal).
    Tui,
    /// Print the version and wire protocol versions.
    Version {
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Subcommand)]
pub enum AttachCmd {
    /// Run the Herdr client; tab/pane focus from any client is applied to all.
    Herdr {
        /// Named Herdr session (`herdr session attach <name>`).
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Debug, Clone, Args)]
pub struct PollArgs {
    /// Named Herdr session (default: Herdr's own resolution).
    #[arg(long)]
    pub session: Option<String>,
    /// The `rev` the caller already has: answer `{"rev","unchanged":true}`
    /// while the state still matches it.
    #[arg(long)]
    pub since: Option<String>,
    /// With `--since`, wait up to this many milliseconds for a change
    /// (capped at 20000), re-reading Herdr every 250 ms.
    #[arg(long, default_value_t = 0)]
    pub wait_ms: u64,
}

#[derive(Debug, Subcommand)]
pub enum HerdrCmd {
    /// Workspaces, tabs and panes with agent status, plus their `rev`.
    Snapshot {
        #[command(flatten)]
        poll: PollArgs,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// A pane's own text (escape sequences and control characters removed)
    /// with its agent and status.
    Read {
        /// Pane id, e.g. `w1:p2`.
        #[arg(long, allow_hyphen_values = true)]
        pane: String,
        /// Last lines to return (1..=2000).
        #[arg(long, default_value_t = crate::features::navigator::domain::DEFAULT_LINES)]
        lines: u32,
        #[command(flatten)]
        poll: PollArgs,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Run one allowlisted action; the request JSON `{"op":…}` comes on stdin.
    Action {
        /// Named Herdr session (default: Herdr's own resolution).
        #[arg(long)]
        session: Option<String>,
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Subcommand)]
pub enum ListCmd {
    List {
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Subcommand)]
pub enum InboxCmd {
    List {
        /// Only events whose seq is greater than this.
        #[arg(long)]
        since: Option<u64>,
        /// Maximum number of events (1..=500, default 100).
        #[arg(long)]
        limit: Option<u32>,
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ActionArg {
    Allow,
    Deny,
    Text,
}

#[derive(Debug, Subcommand)]
pub enum DevicesCmd {
    List {
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Remove a device's SSH key and push registration.
    Revoke {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Rotate a device's push endpoint.
    SetEndpoint {
        #[arg(allow_hyphen_values = true)]
        id: String,
        url: String,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Register a Web Push subscription (one JSON object on stdin).
    SetWebpush {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Remove a device's Web Push subscription.
    ClearWebpush {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KindArg {
    #[value(name = "approval_required")]
    ApprovalRequired,
    Question,
    Done,
    /// Approval when the payload is about a permission, else question.
    Notification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentArg {
    /// Claude Code (~/.claude/settings.json)
    Claude,
    /// OpenAI Codex CLI (~/.codex/hooks.json)
    Codex,
    /// Gemini CLI (~/.gemini/settings.json)
    Gemini,
    /// Cursor agent (~/.cursor/hooks.json)
    Cursor,
    /// GitHub Copilot CLI (~/.copilot/hooks/openmosh.json)
    Copilot,
    /// Kiro CLI (~/.kiro/hooks/openmosh.json)
    Kiro,
    /// OpenCode (~/.config/opencode/plugins/openmosh.js)
    Opencode,
    /// Aider (~/.aider.conf.yml)
    Aider,
}

#[derive(Debug, Subcommand)]
pub enum IntegrationCmd {
    /// Wire the agent's hooks to `openmosh-hook emit`.
    Install {
        agent: AgentArg,
        /// File to edit (default: the agent's user-level file listed above).
        #[arg(long)]
        settings: Option<PathBuf>,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Remove only the hooks openmosh-hook added.
    Uninstall {
        agent: AgentArg,
        #[arg(long)]
        settings: Option<PathBuf>,
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Every supported agent: detected on this machine, integration installed.
    List {
        #[command(flatten)]
        out: JsonFlag,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCmd {
    Install {
        /// Print the plan without writing files or running commands.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        out: JsonFlag,
    },
    Uninstall {
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        out: JsonFlag,
    },
}

impl Command {
    /// Whether errors must be printed as a JSON envelope.
    pub fn json(&self) -> bool {
        match self {
            Command::Serve | Command::Pair { .. } | Command::Attach { .. } | Command::Tui => false,
            Command::Emit { out, .. }
            | Command::PairSsh { out }
            | Command::Respond { out, .. }
            | Command::Doctor { out }
            | Command::HardenSsh { out, .. }
            | Command::Setup { out, .. }
            | Command::Uninstall { out, .. }
            | Command::Usage { out, .. }
            | Command::Version { out } => out.json,
            Command::Sessions {
                cmd: ListCmd::List { out },
            }
            | Command::Agents {
                cmd: ListCmd::List { out },
            } => out.json,
            Command::Inbox {
                cmd: InboxCmd::List { out, .. },
            } => out.json,
            Command::Devices { cmd } => match cmd {
                DevicesCmd::List { out }
                | DevicesCmd::Revoke { out, .. }
                | DevicesCmd::SetEndpoint { out, .. }
                | DevicesCmd::SetWebpush { out, .. }
                | DevicesCmd::ClearWebpush { out, .. } => out.json,
            },
            Command::Integration { cmd } => match cmd {
                IntegrationCmd::Install { out, .. }
                | IntegrationCmd::Uninstall { out, .. }
                | IntegrationCmd::List { out } => out.json,
            },
            Command::Service { cmd } => match cmd {
                ServiceCmd::Install { out, .. } | ServiceCmd::Uninstall { out, .. } => out.json,
            },
            Command::Herdr { cmd } => match cmd {
                HerdrCmd::Snapshot { out, .. }
                | HerdrCmd::Read { out, .. }
                | HerdrCmd::Action { out, .. } => out.json,
            },
        }
    }
}
