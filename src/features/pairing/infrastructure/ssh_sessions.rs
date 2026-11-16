//! `LiveSessions` over `/proc` (Linux): finds this user's processes whose
//! `SSH_CONNECTION` peer is one of the given addresses and ends the SSH
//! session process (`sshd` / `sshd-session`) they run under with `kill -TERM`.
//! The caller's own process chain is never touched, so revoking a phone from
//! a session on that same phone does not cut the command off half-way.
//! Best effort: unreadable processes are skipped; without `/proc` nothing is
//! inspected and `None` is returned.

use std::collections::BTreeSet;
use std::fs;
use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::features::pairing::domain::LiveSessions;
use crate::shared::domain::command::CommandRunner;

pub struct ProcSshSessions<'a> {
    pub proc_root: PathBuf,
    /// Only processes of this user are looked at or signalled.
    pub uid: u32,
    /// Process ids that must survive (the caller and its ancestors).
    pub protect: Vec<u32>,
    pub runner: &'a dyn CommandRunner,
}

/// `ppid` and `comm` from `/proc/<pid>/stat` (comm may contain spaces and parens).
fn stat(proc_root: &Path, pid: u32) -> Option<(u32, String)> {
    let s = fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let comm = s.get(open + 1..close)?.to_string();
    let ppid = s
        .get(close + 1..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((ppid, comm))
}

/// The process and all its ancestors.
pub fn ancestry(proc_root: &Path, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut cur = pid;
    while let Some((ppid, _)) = stat(proc_root, cur) {
        if ppid <= 1 || out.contains(&ppid) || out.len() > 64 {
            break;
        }
        out.push(ppid);
        cur = ppid;
    }
    out
}

/// The peer address in `SSH_CONNECTION=<peer ip> <port> <local ip> <port>`.
fn ssh_peer(environ: &[u8]) -> Option<IpAddr> {
    environ
        .split(|b| *b == 0)
        .filter_map(|kv| std::str::from_utf8(kv).ok())
        .find_map(|kv| kv.strip_prefix("SSH_CONNECTION="))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|ip| ip.parse::<IpAddr>().ok())
        .map(|ip| ip.to_canonical())
}

impl ProcSshSessions<'_> {
    fn owned(&self, pid: u32) -> bool {
        fs::metadata(self.proc_root.join(pid.to_string())).is_ok_and(|m| m.uid() == self.uid)
    }

    /// The nearest `sshd`/`sshd-session` ancestor owned by the user: the
    /// process that holds this connection. None when there is none: a daemon
    /// forked from the session (tmux or Herdr server, `nohup` job) inherits
    /// `SSH_CONNECTION` but must not be killed with it.
    fn session_of(&self, pid: u32) -> Option<u32> {
        let mut cur = pid;
        for _ in 0..64 {
            let (ppid, _) = stat(&self.proc_root, cur)?;
            if ppid <= 1 || !self.owned(ppid) {
                return None;
            }
            if stat(&self.proc_root, ppid).is_some_and(|(_, c)| c.starts_with("sshd")) {
                return Some(ppid);
            }
            cur = ppid;
        }
        None
    }
}

impl LiveSessions for ProcSshSessions<'_> {
    fn end_sessions_from(&self, ips: &[IpAddr]) -> Option<Vec<u32>> {
        let entries = fs::read_dir(&self.proc_root).ok()?;
        let ips: Vec<IpAddr> = ips.iter().map(|i| i.to_canonical()).collect();
        let mut targets = BTreeSet::new();
        for e in entries.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            if pid <= 1 || self.protect.contains(&pid) || !self.owned(pid) {
                continue;
            }
            let Ok(env) = fs::read(e.path().join("environ")) else {
                continue;
            };
            if ssh_peer(&env).is_some_and(|p| ips.contains(&p)) {
                if let Some(s) = self.session_of(pid).filter(|s| !self.protect.contains(s)) {
                    targets.insert(s);
                }
            }
        }
        let mut ended = Vec::new();
        for pid in targets {
            let args = ["-TERM".to_string(), pid.to_string()];
            if self.runner.run("kill", &args).is_ok_and(|o| o.success()) {
                ended.push(pid);
            }
        }
        Some(ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;

    fn proc(root: &Path, pid: u32, ppid: u32, comm: &str, env: &str) {
        let d = root.join(pid.to_string());
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("stat"), format!("{pid} ({comm}) S {ppid} 1 1 0")).unwrap();
        fs::write(d.join("environ"), env.replace(' ', "\0").replace('~', " ")).unwrap();
    }

    fn conn(ip: &str) -> String {
        format!("HOME=/h SSH_CONNECTION={ip}~5555~100.88.77.66~22 TERM=xterm")
    }

    #[test]
    fn ends_only_the_phones_sessions_and_never_the_callers() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let uid = fs::metadata(root).unwrap().uid();
        proc(root, 10, 1, "sshd", "");
        // The phone's session: sshd-session -> bash.
        proc(root, 11, 10, "sshd-session", "");
        proc(root, 12, 11, "bash", &conn("100.101.1.2"));
        proc(root, 13, 12, "falconer-hook", &conn("100.101.1.2"));
        // Another device.
        proc(root, 20, 10, "sshd: me@pts/1", "");
        proc(root, 21, 20, "zsh", &conn("100.101.9.9"));
        // The caller runs over the phone's own address: protected.
        proc(root, 30, 10, "sshd", "");
        proc(root, 31, 30, "sh (x) y", &conn("::ffff:100.101.1.2"));
        // Daemons forked from the phone's session (a tmux or Herdr server,
        // reparented to init) carry its SSH_CONNECTION: never killed.
        proc(root, 40, 1, "tmux: server", &conn("100.101.1.2"));
        proc(root, 41, 40, "zsh", &conn("100.101.1.2"));
        let r = RecordingRunner::default();
        let s = ProcSshSessions {
            proc_root: root.to_path_buf(),
            uid,
            protect: ancestry(root, 31),
            runner: &r,
        };
        assert_eq!(ancestry(root, 31), vec![31, 30, 10]);
        let ended = s.end_sessions_from(&["100.101.1.2".parse().unwrap()]);
        assert_eq!(ended, Some(vec![11]));
        assert_eq!(r.argv(), vec![vec!["kill", "-TERM", "11"]]);
    }

    #[test]
    fn without_proc_nothing_is_known() {
        let r = RecordingRunner::default();
        let s = ProcSshSessions {
            proc_root: "/nonexistent/proc".into(),
            uid: 0,
            protect: vec![],
            runner: &r,
        };
        assert_eq!(s.end_sessions_from(&["100.101.1.2".parse().unwrap()]), None);
        assert!(r.argv().is_empty());
    }

    #[test]
    fn reads_the_ssh_peer() {
        assert_eq!(
            ssh_peer(b"A=1\0SSH_CONNECTION=fd7a:115c:a1e0::1 22 x 22\0"),
            Some("fd7a:115c:a1e0::1".parse().unwrap())
        );
        assert_eq!(ssh_peer(b"SSH_CLIENT=1.2.3.4 1 22\0"), None);
    }
}
