//! Every screen action with its keys and labels. The same table drives key
//! dispatch, the footer hints, the `?` help and the `/` palette, so a key is
//! never shown without doing something.

use ratatui::crossterm::event::KeyCode;

use super::state::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Enter on a screen with a list of things to do.
    Activate,
    Fix,
    FixAll,
    Wizard,
    Update,
    Allow,
    Deny,
    Reply,
    Pair,
    InvertQr,
    Revoke,
    SetWebpush,
    ClearWebpush,
    IntegrationInstall,
    IntegrationUninstall,
    ServiceInstall,
    ServiceRestart,
    ServiceUninstall,
    SetEndpoint,
    MoreDays,
    FewerDays,
    HardenSsh,
    Setup,
    SetupAll,
    LegacyCleanup,
    ToggleAll,
    Uninstall,
}

pub struct ActionDef {
    pub screen: Screen,
    pub act: Act,
    /// Keys that run it; empty for actions reached from a list or the palette.
    pub keys: &'static [KeyCode],
    /// The keys as shown to people.
    pub key: &'static str,
    /// Short footer hint.
    pub hint: &'static str,
    /// Help and palette text.
    pub label: &'static str,
}

const fn a(
    screen: Screen,
    act: Act,
    keys: &'static [KeyCode],
    key: &'static str,
    hint: &'static str,
    label: &'static str,
) -> ActionDef {
    ActionDef {
        screen,
        act,
        keys,
        key,
        hint,
        label,
    }
}

use KeyCode::{Char, Delete, Enter};
use Screen as S;

#[rustfmt::skip]
pub const ACTIONS: &[ActionDef] = &[
    a(S::Home, Act::Activate, &[Enter], "Enter", "do", "Do the selected item"),
    a(S::Home, Act::Fix, &[Char('f')], "f", "fix", "Fix the selected problem"),
    a(S::Home, Act::FixAll, &[Char('F')], "F", "fix all", "Install what's missing"),
    a(S::Home, Act::Wizard, &[Char('w')], "w", "wizard", "Run setup wizard"),
    a(S::Home, Act::Update, &[Char('u')], "u", "update", "Update Falconer"),
    a(S::Agents, Act::Allow, &[Char('a')], "a", "allow", "Allow the selected request"),
    a(S::Agents, Act::Deny, &[Char('d')], "d", "deny", "Deny the selected request"),
    a(S::Agents, Act::Reply, &[Char('t')], "t", "reply", "Reply with text"),
    a(S::Phones, Act::Pair, &[Char('p')], "p", "pair", "Pair a phone"),
    a(S::Phones, Act::InvertQr, &[Char('i')], "i", "invert QR", "Invert QR colors"),
    a(S::Phones, Act::Revoke, &[Char('x'), Delete], "x", "revoke", "Revoke the selected phone"),
    a(S::Phones, Act::SetWebpush, &[Char('w')], "w", "web push", "Set the Web Push subscription"),
    a(S::Phones, Act::ClearWebpush, &[Char('c')], "c", "clear push", "Clear the Web Push subscription"),
    a(S::Integrations, Act::IntegrationInstall, &[Char('i')], "i", "install", "Install the selected integration"),
    a(S::Integrations, Act::IntegrationUninstall, &[Char('u')], "u", "uninstall", "Uninstall the selected integration"),
    a(S::Service, Act::ServiceInstall, &[Char('i')], "i", "install", "Install the user service"),
    a(S::Service, Act::ServiceRestart, &[Char('s')], "s", "restart", "Restart the service"),
    a(S::Service, Act::ServiceUninstall, &[Char('u')], "u", "uninstall", "Uninstall the user service"),
    a(S::Settings, Act::SetEndpoint, &[Char('e')], "e", "endpoint", "Set the push endpoint of the selected phone"),
    a(S::Usage, Act::MoreDays, &[Char('+'), Char('=')], "+", "more days", "Show more days"),
    a(S::Usage, Act::FewerDays, &[Char('-')], "-", "fewer days", "Show fewer days"),
    a(S::Advanced, Act::Activate, &[Enter], "Enter", "run", "Run the selected action"),
    a(S::Advanced, Act::HardenSsh, &[Char('s')], "s", "SSH keys only", "SSH: keys only, no passwords (sudo)"),
    a(S::Advanced, Act::ToggleAll, &[Char('a')], "a", "toggle --all", "Toggle --all (also delete the binary)"),
    a(S::Advanced, Act::Uninstall, &[Char('u')], "u", "uninstall", "Uninstall Falconer"),
    a(S::Advanced, Act::Setup, &[], "", "", "Run setup (integrations as they are)"),
    a(S::Advanced, Act::SetupAll, &[], "", "", "Run setup and install every detected integration"),
    a(S::Advanced, Act::LegacyCleanup, &[], "", "", "Clean up OpenMosh leftovers"),
];

/// The action `code` runs on `screen`.
pub fn for_key(screen: Screen, code: KeyCode) -> Option<Act> {
    ACTIONS
        .iter()
        .find(|d| d.screen == screen && d.keys.contains(&code))
        .map(|d| d.act)
}

/// The actions of `screen`, in table order.
pub fn on(screen: Screen) -> impl Iterator<Item = &'static ActionDef> {
    ACTIONS.iter().filter(move |d| d.screen == screen)
}

/// The definition of `act` (its label), first match.
pub fn def(act: Act) -> &'static ActionDef {
    ACTIONS.iter().find(|d| d.act == act).unwrap_or(&ACTIONS[0])
}
