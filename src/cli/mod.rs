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
    #[command(subcommand)]
    pub command: Command,
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
    /// Check Tailscale, Herdr/tmux, permissions, devices and the service.
    Doctor {
        #[command(flatten)]
        out: JsonFlag,
    },
    /// Register or remove the user service.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Run a multiplexer client with focus mirroring across attached clients.
    Attach {
        #[command(subcommand)]
        cmd: AttachCmd,
    },
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

#[derive(Debug, Clone, Copy, ValueEnum)]
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
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AgentArg {
    Claude,
}

#[derive(Debug, Subcommand)]
pub enum IntegrationCmd {
    Install {
        agent: AgentArg,
        /// Settings file (default: ~/.claude/settings.json).
        #[arg(long)]
        settings: Option<PathBuf>,
        #[command(flatten)]
        out: JsonFlag,
    },
    Uninstall {
        agent: AgentArg,
        #[arg(long)]
        settings: Option<PathBuf>,
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
            Command::Serve | Command::Pair { .. } | Command::Attach { .. } => false,
            Command::Emit { out, .. }
            | Command::PairSsh { out }
            | Command::Respond { out, .. }
            | Command::Doctor { out }
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
                | DevicesCmd::SetEndpoint { out, .. } => out.json,
            },
            Command::Integration { cmd } => match cmd {
                IntegrationCmd::Install { out, .. } | IntegrationCmd::Uninstall { out, .. } => {
                    out.json
                }
            },
            Command::Service { cmd } => match cmd {
                ServiceCmd::Install { out, .. } | ServiceCmd::Uninstall { out, .. } => out.json,
            },
        }
    }
}
