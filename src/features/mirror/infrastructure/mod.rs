//! Adapters: Herdr's newline-delimited JSON socket, the TTY child launcher
//! and the monotonic clock.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::features::mirror::domain::{
    valid_session_name, ChildProcess, FocusKind, FocusSnapshot, FocusSocket, FocusTarget, Launcher,
    MirrorError, MonoClock, SocketEvent,
};

/// Longest accepted protocol line; anything larger is treated as a broken stream.
const MAX_LINE: usize = 1 << 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Herdr's socket path, in Herdr's own resolution order: `--session`,
/// `HERDR_SOCKET_PATH` (absolute only), `HERDR_SESSION`, then the default.
pub fn socket_path(
    session: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
    xdg_config_home: &Path,
) -> PathBuf {
    let base = xdg_config_home.join("herdr");
    // `default` is Herdr's name for the default session, served on the base socket.
    let named = |name: &str| {
        if name == "default" {
            base.join("herdr.sock")
        } else {
            base.join("sessions").join(name).join("herdr.sock")
        }
    };
    if let Some(name) = session.filter(|n| valid_session_name(n)) {
        return named(name);
    }
    if let Some(p) = env("HERDR_SOCKET_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return p;
    }
    if let Some(name) = env("HERDR_SESSION").filter(|n| valid_session_name(n)) {
        return named(&name);
    }
    base.join("herdr.sock")
}

/// Parses one subscription line. `Ok(None)` for lines the mirror ignores.
pub fn parse_event_line(line: &str) -> Result<Option<SocketEvent>, MirrorError> {
    let v: Value = serde_json::from_str(line).map_err(|_| MirrorError::Protocol("bad_json"))?;
    if let Some(err) = v.get("error") {
        return match err.get("code").and_then(Value::as_str) {
            Some("events_lost") => Ok(Some(SocketEvent::EventsLost)),
            _ => Err(MirrorError::Protocol("subscription_error")),
        };
    }
    let Some(kind) = v.get("event").and_then(Value::as_str) else {
        return Ok(None);
    };
    let data = v.get("data").unwrap_or(&Value::Null);
    let (kind, field) = match kind.replace('.', "_").as_str() {
        "workspace_focused" => (FocusKind::Workspace, "workspace_id"),
        "tab_focused" => (FocusKind::Tab, "tab_id"),
        "pane_focused" => (FocusKind::Pane, "pane_id"),
        _ => return Ok(None),
    };
    Ok(data
        .get(field)
        .and_then(Value::as_str)
        .and_then(|id| FocusTarget::new(kind, id))
        .map(SocketEvent::Focused))
}

/// Focus fields of a `session.snapshot` result.
pub fn parse_snapshot(result: &Value) -> FocusSnapshot {
    let s = result.get("snapshot").unwrap_or(&Value::Null);
    let id = |k: &str| {
        s.get(k)
            .and_then(Value::as_str)
            .filter(|v| crate::features::mirror::domain::valid_target_id(v))
            .map(str::to_string)
    };
    FocusSnapshot {
        workspace: id("focused_workspace_id"),
        tab: id("focused_tab_id"),
        pane: id("focused_pane_id"),
    }
}

/// Connects to Herdr's socket; with `owner`, only to a socket that user owns
/// (another user's socket on a shared path could feed fake data or receive ours).
pub fn open_socket(path: &Path, owner: Option<u32>) -> Result<UnixStream, MirrorError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    if let Some(uid) = owner {
        let m = std::fs::metadata(path).map_err(|_| MirrorError::Unavailable)?;
        if m.uid() != uid || !m.file_type().is_socket() {
            return Err(MirrorError::Unavailable);
        }
    }
    let s = UnixStream::connect(path).map_err(|_| MirrorError::Unavailable)?;
    s.set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|_| MirrorError::Unavailable)?;
    Ok(s)
}

/// One JSON-RPC request on its own connection (Herdr 0.9 closes a request
/// connection after one reply); returns the reply's `result`. Lines with
/// another id are skipped; an error reply becomes `Refused(code)`.
pub fn request_once(
    path: &Path,
    owner: Option<u32>,
    id: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, MirrorError> {
    let line = json!({"id": id, "method": method, "params": params}).to_string();
    let mut stream = open_socket(path, owner)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| MirrorError::Closed)?;
    writeln!(stream, "{line}").map_err(|_| MirrorError::Closed)?;
    let mut cmd = BufReader::new(stream);
    loop {
        let mut reply = String::new();
        let n = cmd
            .by_ref()
            .take(MAX_LINE as u64)
            .read_line(&mut reply)
            .map_err(|_| MirrorError::Closed)?;
        if n == 0 || !reply.ends_with('\n') {
            return Err(MirrorError::Closed);
        }
        let v: Value =
            serde_json::from_str(&reply).map_err(|_| MirrorError::Protocol("bad_json"))?;
        if v.get("id").and_then(Value::as_str) != Some(id) {
            continue;
        }
        if let Some(err) = v.get("error") {
            let code: String = err
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(64)
                .collect();
            return Err(MirrorError::Refused(code.to_ascii_lowercase()));
        }
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    }
}

/// Unix-socket client: one long-lived subscription; every request opens its
/// own connection, because Herdr 0.9 closes a request connection after one
/// reply.
pub struct HerdrSocket {
    path: PathBuf,
    sub: Option<UnixStream>,
    buf: Vec<u8>,
    next_id: u64,
    owner: Option<u32>,
}

impl HerdrSocket {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            sub: None,
            buf: Vec::new(),
            next_id: 0,
            owner: None,
        }
    }

    /// Only talks to a socket owned by `uid` (another user's socket on a
    /// shared path could feed fake focus events or receive ours).
    pub fn owned_by(mut self, uid: u32) -> Self {
        self.owner = Some(uid);
        self
    }

    fn open(&self) -> Result<UnixStream, MirrorError> {
        open_socket(&self.path, self.owner)
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("falconer-{prefix}-{}", self.next_id)
    }

    /// Sends one request on a fresh connection and returns its `result`.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, MirrorError> {
        if self.sub.is_none() {
            return Err(MirrorError::Closed);
        }
        let id = self.id("req");
        request_once(&self.path, self.owner, &id, method, params, REQUEST_TIMEOUT).map_err(|e| {
            match e {
                MirrorError::Refused(_) => MirrorError::Protocol("request_refused"),
                MirrorError::Unavailable => MirrorError::Closed,
                other => other,
            }
        })
    }

    /// Reads from the subscription until a full line is buffered or `wait` passes.
    fn read_line(&mut self, wait: Duration) -> Result<Option<String>, MirrorError> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let line = String::from_utf8(line).map_err(|_| MirrorError::Protocol("utf8"))?;
                return Ok(Some(line));
            }
            if self.buf.len() > MAX_LINE {
                return Err(MirrorError::Protocol("line_too_long"));
            }
            let sub = self.sub.as_mut().ok_or(MirrorError::Closed)?;
            sub.set_read_timeout(Some(wait.max(Duration::from_millis(1))))
                .map_err(|_| MirrorError::Closed)?;
            let mut chunk = [0u8; 4096];
            match sub.read(&mut chunk) {
                Ok(0) => return Err(MirrorError::Closed),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None)
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(MirrorError::Closed),
            }
        }
    }
}

impl FocusSocket for HerdrSocket {
    fn connect(&mut self) -> Result<(), MirrorError> {
        self.sub = None;
        self.buf.clear();
        let mut sub = self.open()?;
        let id = self.id("sub");
        let req = json!({
            "id": id,
            "method": "events.subscribe",
            "params": {"subscriptions": [
                {"type": "workspace.focused"},
                {"type": "tab.focused"},
                {"type": "pane.focused"}
            ]}
        });
        writeln!(sub, "{req}").map_err(|_| MirrorError::Unavailable)?;
        self.sub = Some(sub);
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.sub = None;
                return Err(MirrorError::Protocol("subscribe_timeout"));
            }
            let Some(line) = self.read_line(left)? else {
                continue;
            };
            let v: Value =
                serde_json::from_str(&line).map_err(|_| MirrorError::Protocol("bad_json"))?;
            if v.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            let started =
                v.pointer("/result/type").and_then(Value::as_str) == Some("subscription_started");
            if !started {
                self.sub = None;
                return Err(MirrorError::Protocol("subscribe_refused"));
            }
            break;
        }
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Result<Option<SocketEvent>, MirrorError> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(line) = self.read_line(left)? else {
                return Ok(None);
            };
            if let Some(ev) = parse_event_line(&line)? {
                return Ok(Some(ev));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    fn snapshot(&mut self) -> Result<FocusSnapshot, MirrorError> {
        let result = self.request("session.snapshot", json!({}))?;
        Ok(parse_snapshot(&result))
    }

    fn focus(&mut self, target: &FocusTarget) -> Result<(), MirrorError> {
        let (method, field) = match target.kind {
            FocusKind::Workspace => ("workspace.focus", "workspace_id"),
            FocusKind::Tab => ("tab.focus", "tab_id"),
            FocusKind::Pane => ("pane.focus", "pane_id"),
        };
        self.request(method, json!({ field: target.id }))
            .map(|_| ())
    }
}

/// Spawns a program that shares this process's terminal (argv only, no shell).
pub struct TtyLauncher;

pub struct TtyChild(std::process::Child, String);

impl Launcher for TtyLauncher {
    type Child = TtyChild;
    fn spawn(&self, program: &str, args: &[String]) -> Result<TtyChild, MirrorError> {
        Command::new(program)
            .args(args)
            .spawn()
            .map(|c| TtyChild(c, program.to_string()))
            .map_err(|_| MirrorError::Launch(program.to_string()))
    }
}

impl ChildProcess for TtyChild {
    fn wait(&mut self) -> Result<i32, MirrorError> {
        let status = self
            .0
            .wait()
            .map_err(|_| MirrorError::Launch(self.1.clone()))?;
        Ok(status
            .code()
            .or_else(|| status.signal().map(|s| 128 + s))
            .unwrap_or(1))
    }
}

/// Real monotonic clock.
pub struct SystemMonoClock(Instant);

impl Default for SystemMonoClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl MonoClock for SystemMonoClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests;
