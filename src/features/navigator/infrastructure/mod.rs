//! Navigator adapter: Herdr's JSON-RPC socket (verified against herdr 0.9.0,
//! protocol 22).
//!
//! - `session.snapshot` → `result.snapshot {workspaces, tabs, panes, …}`
//! - `pane.read {pane_id, source:"recent", lines, format:"text", strip_ansi}` → `result.read.text`
//! - `workspace.create` → `result.workspace.workspace_id`; `tab.create` → `result.tab.tab_id`
//! - `agent.start {name, kind, pane_id, timeout_ms}` waits until the agent is ready
//! - `pane.send_keys {pane_id, keys}` works on any pane (the CLI's
//!   `agent send-keys` needs a detected agent)

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

use crate::features::agents::domain::AgentStatus;
use crate::features::mirror::domain::{valid_target_id, MirrorError};
use crate::features::mirror::infrastructure::request_once;
use crate::features::navigator::domain::{
    NavError, NavPane, NavSnapshot, NavTab, NavWorkspace, NavigatorApi, Op,
};
use crate::shared::domain::text::clip;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// `agent.start` readiness wait, and the socket wait around it.
const AGENT_START_MS: u64 = 20_000;
const AGENT_START_TIMEOUT: Duration = Duration::from_secs(25);

impl From<MirrorError> for NavError {
    fn from(e: MirrorError) -> Self {
        match e {
            MirrorError::Unavailable | MirrorError::Closed | MirrorError::Launch(_) => {
                NavError::Unavailable
            }
            MirrorError::Protocol(p) => NavError::Protocol(p),
            MirrorError::Refused(code) => NavError::Refused(code),
        }
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn id_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|id| valid_target_id(id))
        .map(str::to_string)
}

fn status(v: &Value) -> AgentStatus {
    AgentStatus::parse(s(v, "agent_status"))
}

fn flag(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn num(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

fn items<'a>(v: &'a Value, k: &str) -> impl Iterator<Item = &'a Value> {
    v.get(k).and_then(Value::as_array).into_iter().flatten()
}

/// Maps a `session.snapshot` result. Entries with ids that are not plain
/// Herdr ids are dropped; labels become one clean line of at most 64 chars.
pub fn parse_nav_snapshot(result: &Value) -> NavSnapshot {
    let snap = result.get("snapshot").unwrap_or(&Value::Null);
    let workspaces = items(snap, "workspaces")
        .filter_map(|w| {
            Some(NavWorkspace {
                id: id_of(w, "workspace_id")?,
                label: clip(s(w, "label"), 64),
                focused: flag(w, "focused"),
            })
        })
        .collect();
    let tabs = items(snap, "tabs")
        .filter_map(|t| {
            Some(NavTab {
                id: id_of(t, "tab_id")?,
                workspace_id: id_of(t, "workspace_id")?,
                label: clip(s(t, "label"), 64),
                number: num(t, "number"),
                agent_status: status(t),
                pane_count: num(t, "pane_count"),
                focused: flag(t, "focused"),
            })
        })
        .collect();
    let panes = items(snap, "panes")
        .filter_map(|p| {
            Some(NavPane {
                id: id_of(p, "pane_id")?,
                tab_id: id_of(p, "tab_id")?,
                workspace_id: id_of(p, "workspace_id")?,
                agent: p
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(|a| clip(a, 32))
                    .filter(|a| !a.is_empty()),
                agent_status: status(p),
                focused: flag(p, "focused"),
            })
        })
        .collect();
    NavSnapshot {
        workspaces,
        tabs,
        panes,
    }
}

/// The socket method, params and wait for a socket action; `None` for
/// `pane.text`, which is not sent over the socket.
pub fn socket_call(op: &Op) -> Option<(&'static str, Value, Duration)> {
    let t = REQUEST_TIMEOUT;
    Some(match op {
        Op::WorkspaceCreate(w) => (
            "workspace.create",
            json!({"label": w.label, "focus": false}),
            t,
        ),
        Op::WorkspaceRename(r) => (
            "workspace.rename",
            json!({"workspace_id": r.id, "label": r.label}),
            t,
        ),
        Op::WorkspaceClose(x) => ("workspace.close", json!({"workspace_id": x.id}), t),
        Op::WorkspaceFocus(x) => ("workspace.focus", json!({"workspace_id": x.id}), t),
        Op::TabCreate(n) => (
            "tab.create",
            json!({"workspace_id": n.workspace_id, "label": n.label, "focus": false}),
            t,
        ),
        Op::TabRename(r) => ("tab.rename", json!({"tab_id": r.id, "label": r.label}), t),
        Op::TabClose(x) => ("tab.close", json!({"tab_id": x.id}), t),
        Op::TabFocus(x) => ("tab.focus", json!({"tab_id": x.id}), t),
        Op::PaneFocus(x) => ("pane.focus", json!({"pane_id": x.id}), t),
        Op::AgentStart(a) => (
            "agent.start",
            json!({"name": a.agent, "kind": a.agent, "pane_id": a.pane_id,
                   "timeout_ms": AGENT_START_MS}),
            AGENT_START_TIMEOUT,
        ),
        Op::PaneKeys(k) => (
            "pane.send_keys",
            json!({"pane_id": k.pane_id, "keys": k.keys}),
            t,
        ),
        Op::PaneText(_) => return None,
    })
}

/// The id an action created, from its result.
fn created_id(op: &Op, result: &Value) -> Option<String> {
    let ptr = match op {
        Op::WorkspaceCreate(_) => "/workspace/workspace_id",
        Op::TabCreate(_) => "/tab/tab_id",
        Op::AgentStart(_) => "/agent/pane_id",
        _ => return None,
    };
    result
        .pointer(ptr)
        .and_then(Value::as_str)
        .filter(|id| valid_target_id(id))
        .map(str::to_string)
}

/// One request per connection on the session's socket, owned by `owner`.
pub struct SocketNavigator {
    pub path: PathBuf,
    pub owner: Option<u32>,
    next_id: u64,
}

impl SocketNavigator {
    pub fn new(path: PathBuf, owner: Option<u32>) -> Self {
        Self {
            path,
            owner,
            next_id: 0,
        }
    }

    fn call(&mut self, method: &str, params: Value, wait: Duration) -> Result<Value, NavError> {
        self.next_id += 1;
        let id = format!("openmosh-nav-{}", self.next_id);
        Ok(request_once(
            &self.path, self.owner, &id, method, params, wait,
        )?)
    }
}

impl NavigatorApi for SocketNavigator {
    fn snapshot(&mut self) -> Result<NavSnapshot, NavError> {
        let r = self.call("session.snapshot", json!({}), REQUEST_TIMEOUT)?;
        if r.get("snapshot").is_none() {
            return Err(NavError::Protocol("no_snapshot"));
        }
        Ok(parse_nav_snapshot(&r))
    }

    fn read_pane(&mut self, pane_id: &str, lines: u32) -> Result<String, NavError> {
        let r = self.call(
            "pane.read",
            json!({"pane_id": pane_id, "source": "recent", "lines": lines,
                   "format": "text", "strip_ansi": true}),
            REQUEST_TIMEOUT,
        )?;
        r.pointer("/read/text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(NavError::Protocol("no_text"))
    }

    fn apply(&mut self, op: &Op) -> Result<Option<String>, NavError> {
        let (method, params, wait) =
            socket_call(op).ok_or(NavError::Protocol("not_a_socket_action"))?;
        let r = self.call(method, params, wait)?;
        Ok(created_id(op, &r))
    }
}

#[cfg(test)]
mod tests;
