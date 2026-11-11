//! Herdr navigator: the structured view of a Herdr session the phone
//! browses, the fixed allowlist of actions it may request, their
//! validation, revisions for long-polling and the port to Herdr.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::features::agents::domain::AgentStatus;
use crate::features::mirror::domain::valid_target_id;
use crate::shared::domain::error::ErrorCode;
use crate::shared::domain::runtime::valid_pane_id;
use crate::shared::domain::text::is_spoofing_char;

/// Longest `--wait-ms` a call may block (under the app's 30 s exec timeout).
pub const MAX_WAIT_MS: u64 = 20_000;
/// How often a long-poll re-reads Herdr.
pub const POLL_MS: u64 = 250;
/// `herdr read --lines` default and maximum.
pub const DEFAULT_LINES: u32 = 200;
pub const MAX_LINES: u32 = 2000;
/// Workspace and tab labels, in characters.
pub const MAX_LABEL_CHARS: usize = 64;
/// `pane.text` text, in characters (before sanitization).
pub const MAX_TEXT_CHARS: usize = 4000;
/// Keys in one `pane.keys` request.
pub const MAX_KEYS: usize = 8;

/// Key names `pane.keys` accepts.
pub const ALLOWED_KEYS: &[&str] = &[
    "enter",
    "esc",
    "tab",
    "space",
    "backspace",
    "up",
    "down",
    "left",
    "right",
    "y",
    "n",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
];

/// Agents `agent.start` may launch: the hook's agents (integration catalog
/// and keymaps) that `herdr agent start --kind` supports. Aider is not a
/// Herdr agent kind, so it cannot be started this way.
pub const STARTABLE_AGENTS: &[&str] = &[
    "claude", "codex", "gemini", "cursor", "copilot", "kiro", "opencode", "amp", "pi",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NavWorkspace {
    pub id: String,
    pub label: String,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NavTab {
    pub id: String,
    pub workspace_id: String,
    pub label: String,
    pub number: u64,
    pub agent_status: AgentStatus,
    pub pane_count: u64,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NavPane {
    pub id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    pub focused: bool,
}

/// What `herdr snapshot` returns (and hashes into `rev`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NavSnapshot {
    pub workspaces: Vec<NavWorkspace>,
    pub tabs: Vec<NavTab>,
    pub panes: Vec<NavPane>,
}

impl NavSnapshot {
    pub fn has_workspace(&self, id: &str) -> bool {
        self.workspaces.iter().any(|w| w.id == id)
    }
    pub fn has_tab(&self, id: &str) -> bool {
        self.tabs.iter().any(|t| t.id == id)
    }
    pub fn pane(&self, id: &str) -> Option<&NavPane> {
        self.panes.iter().find(|p| p.id == id)
    }
}

/// What `herdr read` returns (and hashes into `rev`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneScreen {
    pub agent: Option<String>,
    pub agent_status: AgentStatus,
    pub lines: Vec<String>,
}

/// A long-poll answer: the body when it changed (or no `--since` was
/// given), otherwise only the revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Polled<T> {
    pub rev: String,
    pub unchanged: bool,
    #[serde(flatten)]
    pub body: Option<T>,
}

/// First 16 hex digits of the SHA-256 of `body`'s JSON (field order is fixed
/// by the structs, so equal content gives an equal revision).
pub fn rev_of<T: Serialize>(body: &T) -> String {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The pane's text as display lines: escape sequences and control or
/// spoofing characters removed, trailing blanks trimmed, the last `max` lines.
pub fn screen_lines(text: &str, max: usize) -> Vec<String> {
    let clean = strip_escapes(text);
    let mut lines: Vec<String> = clean
        .split('\n')
        .map(|l| {
            l.chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !is_spoofing_char(*c))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(max);
    lines.split_off(skip)
}

/// Removes ANSI escape sequences: CSI (`ESC [ … final`), OSC/DCS/PM/APC
/// strings (up to BEL or `ESC \`), and two-byte `ESC x` sequences; also the
/// 8-bit CSI (U+009B).
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.next() {
                Some('[') => skip_csi(&mut it),
                Some(']' | 'P' | '^' | '_' | 'X') => skip_string(&mut it),
                // `ESC ( B` and friends: intermediates, then one final byte.
                Some(' '..='/') => {
                    for c in it.by_ref() {
                        if !(' '..='/').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\u{9b}' => skip_csi(&mut it),
            _ => out.push(c),
        }
    }
    out
}

fn skip_csi(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in it.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            break;
        }
    }
}

fn skip_string(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = it.next() {
        if c == '\u{7}' {
            break;
        }
        if c == '\u{1b}' {
            if it.peek() == Some(&'\\') {
                it.next();
            }
            break;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewWorkspace {
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTab {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartAgent {
    pub pane_id: String,
    pub agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    pub pane_id: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub pane_id: String,
    pub text: String,
    #[serde(default)]
    pub submit: bool,
}

/// One `herdr action` request (`{"op": …, …}` on stdin). The phone can only
/// ask for these; anything else is refused before Herdr is contacted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    #[serde(rename = "workspace.create")]
    WorkspaceCreate(NewWorkspace),
    #[serde(rename = "workspace.rename")]
    WorkspaceRename(Rename),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(Target),
    #[serde(rename = "workspace.focus")]
    WorkspaceFocus(Target),
    #[serde(rename = "tab.create")]
    TabCreate(NewTab),
    #[serde(rename = "tab.rename")]
    TabRename(Rename),
    #[serde(rename = "tab.close")]
    TabClose(Target),
    #[serde(rename = "tab.focus")]
    TabFocus(Target),
    #[serde(rename = "pane.focus")]
    PaneFocus(Target),
    #[serde(rename = "agent.start")]
    AgentStart(StartAgent),
    #[serde(rename = "pane.keys")]
    PaneKeys(Keys),
    #[serde(rename = "pane.text")]
    PaneText(Text),
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::WorkspaceCreate(_) => "workspace.create",
            Op::WorkspaceRename(_) => "workspace.rename",
            Op::WorkspaceClose(_) => "workspace.close",
            Op::WorkspaceFocus(_) => "workspace.focus",
            Op::TabCreate(_) => "tab.create",
            Op::TabRename(_) => "tab.rename",
            Op::TabClose(_) => "tab.close",
            Op::TabFocus(_) => "tab.focus",
            Op::PaneFocus(_) => "pane.focus",
            Op::AgentStart(_) => "agent.start",
            Op::PaneKeys(_) => "pane.keys",
            Op::PaneText(_) => "pane.text",
        }
    }

    /// Parses a request body. Unknown ops and unknown fields are refused.
    pub fn parse(body: &str) -> Result<Op, NavError> {
        serde_json::from_str(body).map_err(|_| NavError::Invalid("unknown op or bad field".into()))
    }
}

fn invalid(msg: &str) -> NavError {
    NavError::Invalid(msg.to_string())
}

/// 1-64 characters, not blank, no control or spoofing characters.
pub fn check_label(label: &str) -> Result<(), NavError> {
    let n = label.chars().count();
    if n == 0 || n > MAX_LABEL_CHARS || label.trim().is_empty() {
        return Err(invalid("label must be 1-64 characters"));
    }
    if label.chars().any(is_spoofing_char) {
        return Err(invalid("label has control characters"));
    }
    Ok(())
}

fn check_workspace(snap: &NavSnapshot, id: &str) -> Result<(), NavError> {
    if !valid_target_id(id) {
        return Err(invalid("invalid workspace id"));
    }
    if !snap.has_workspace(id) {
        return Err(invalid("workspace not found"));
    }
    Ok(())
}

fn check_tab(snap: &NavSnapshot, id: &str) -> Result<(), NavError> {
    if !valid_target_id(id) {
        return Err(invalid("invalid tab id"));
    }
    if !snap.has_tab(id) {
        return Err(invalid("tab not found"));
    }
    Ok(())
}

/// A pane id must pass both the focus-target and the pane validators.
pub fn check_pane(snap: &NavSnapshot, id: &str) -> Result<(), NavError> {
    if !valid_target_id(id) || !valid_pane_id(id) {
        return Err(invalid("invalid pane id"));
    }
    if snap.pane(id).is_none() {
        return Err(invalid("pane not found"));
    }
    Ok(())
}

/// Checks `op` against its limits and the current snapshot. Text is
/// sanitized by the caller's send path, not here.
pub fn validate(op: &Op, snap: &NavSnapshot) -> Result<(), NavError> {
    match op {
        Op::WorkspaceCreate(w) => w.label.as_deref().map_or(Ok(()), check_label),
        Op::WorkspaceRename(r) => {
            check_workspace(snap, &r.id)?;
            check_label(&r.label)
        }
        Op::WorkspaceClose(t) | Op::WorkspaceFocus(t) => check_workspace(snap, &t.id),
        Op::TabCreate(t) => {
            check_workspace(snap, &t.workspace_id)?;
            t.label.as_deref().map_or(Ok(()), check_label)
        }
        Op::TabRename(r) => {
            check_tab(snap, &r.id)?;
            check_label(&r.label)
        }
        Op::TabClose(t) | Op::TabFocus(t) => check_tab(snap, &t.id),
        Op::PaneFocus(t) => check_pane(snap, &t.id),
        Op::AgentStart(a) => {
            check_pane(snap, &a.pane_id)?;
            if !STARTABLE_AGENTS.contains(&a.agent.as_str()) {
                return Err(invalid("unsupported agent"));
            }
            Ok(())
        }
        Op::PaneKeys(k) => {
            check_pane(snap, &k.pane_id)?;
            if k.keys.is_empty() || k.keys.len() > MAX_KEYS {
                return Err(invalid("send 1-8 keys"));
            }
            if let Some(bad) = k.keys.iter().find(|k| !ALLOWED_KEYS.contains(&k.as_str())) {
                let shown: String = bad
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .take(16)
                    .collect();
                return Err(NavError::Invalid(format!("key not allowed: {shown}")));
            }
            Ok(())
        }
        Op::PaneText(t) => {
            check_pane(snap, &t.pane_id)?;
            if t.text.chars().count() > MAX_TEXT_CHARS {
                return Err(invalid("text is longer than 4000 characters"));
            }
            Ok(())
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NavError {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("herdr socket unavailable")]
    Unavailable,
    #[error("herdr refused the request: {0}")]
    Refused(String),
    #[error("herdr protocol error: {0}")]
    Protocol(&'static str),
}

impl ErrorCode for NavError {
    fn code(&self) -> &'static str {
        match self {
            NavError::Invalid(_) => "invalid_request",
            NavError::Unavailable => "runtime_unavailable",
            NavError::Refused(_) | NavError::Protocol(_) => "runtime_error",
        }
    }
}

/// Herdr's socket API, narrowed to what the navigator uses.
pub trait NavigatorApi {
    fn snapshot(&mut self) -> Result<NavSnapshot, NavError>;
    /// The pane's recent text (escape sequences may remain).
    fn read_pane(&mut self, pane_id: &str, lines: u32) -> Result<String, NavError>;
    /// Runs a validated socket action; returns the id of what it created.
    /// `pane.text` is not a socket action (it goes through the CLI send path).
    fn apply(&mut self, op: &Op) -> Result<Option<String>, NavError>;
}

#[cfg(test)]
mod tests;
