//! The first-run setup wizard: six steps, each done, to do or unknown,
//! derived from `doctor` and `integration list`.

use super::state::FixId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Prereqs,
    Ssh,
    Tailscale,
    Integrations,
    Service,
    Pair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Done,
    Todo,
    Unknown,
}

impl WizardStep {
    pub const ALL: [WizardStep; 6] = [
        WizardStep::Prereqs,
        WizardStep::Ssh,
        WizardStep::Tailscale,
        WizardStep::Integrations,
        WizardStep::Service,
        WizardStep::Pair,
    ];

    pub fn title(self) -> &'static str {
        match self {
            WizardStep::Prereqs => "Prerequisites: Herdr or tmux",
            WizardStep::Ssh => "SSH server",
            WizardStep::Tailscale => "Tailscale up",
            WizardStep::Integrations => "Agent integrations",
            WizardStep::Service => "User service",
            WizardStep::Pair => "Pair a phone",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            WizardStep::Prereqs => {
                "An agent runtime: Herdr (recommended, unlocks the agent view in the app) or tmux."
            }
            WizardStep::Ssh => "The phone reaches this machine over SSH, inside your tailnet.",
            WizardStep::Tailscale => {
                "Tailscale must be installed, running and logged in; pairing listens only there."
            }
            WizardStep::Integrations => {
                "Each detected agent gets hooks that call falconer-hook emit; your other hooks stay."
            }
            WizardStep::Service => {
                "The user service runs falconer-hook serve, which watches agents and sends notifications."
            }
            WizardStep::Pair => "Scan a QR code with the Falconer app to add this machine.",
        }
    }

    /// The doctor fixes that complete this step, in order.
    pub fn fixes(self) -> &'static [FixId] {
        match self {
            WizardStep::Prereqs => &[FixId::InstallRuntime],
            WizardStep::Ssh => &[FixId::InstallSsh, FixId::EnableSsh],
            WizardStep::Tailscale => &[FixId::InstallTailscale, FixId::TailscaleUp],
            WizardStep::Integrations => &[],
            WizardStep::Service => &[FixId::InstallService],
            WizardStep::Pair => &[FixId::Pair],
        }
    }
}
