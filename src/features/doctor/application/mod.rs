//! Checks built on other features' ports (read-only calls only).

use crate::features::agents::domain::{AgentError, AgentRuntime};
use crate::features::doctor::domain::Check;
use crate::features::pairing::domain::{DeviceRegistry, Tailnet, TailnetError};
use crate::features::responses::domain::Keymap;

pub fn check_tailscale(t: &impl Tailnet) -> Check {
    match t.status() {
        Err(TailnetError::Missing) => {
            Check::fail("tailscale", "not installed: https://tailscale.com/download")
        }
        Err(TailnetError::Failed) => Check::fail("tailscale", "`tailscale status` failed"),
        Ok(s) if !s.running => Check::fail("tailscale", "not running; run `tailscale up`"),
        Ok(_) => match t.ipv4() {
            Ok(ip) => Check::ok("tailscale", format!("running, {ip}")),
            Err(_) => Check::fail("tailscale", "running but has no IPv4 address"),
        },
    }
}

/// One check per runtime, plus a failure when no runtime is usable.
pub fn check_runtimes(runtimes: &[&dyn AgentRuntime]) -> Vec<Check> {
    let mut out = Vec::new();
    let mut any = false;
    for r in runtimes {
        let name = r.runtime().as_str();
        let check_name = if name == "herdr" { "herdr" } else { "tmux" };
        match r.list_sessions() {
            Ok(s) => {
                any = true;
                out.push(Check::ok(check_name, format!("{} session(s)", s.len())));
            }
            Err(AgentError::Unavailable(_)) => out.push(Check::warn(check_name, "not installed")),
            Err(_) => out.push(Check::warn(check_name, "installed but not responding")),
        }
    }
    if !any {
        out.push(Check::fail(
            "agent_runtime",
            "install Herdr (https://herdr.dev) or tmux",
        ));
    }
    out
}

pub fn check_devices(registry: &impl DeviceRegistry) -> Check {
    match registry.list() {
        Err(e) => Check::fail("devices", e.to_string()),
        Ok(d) if d.is_empty() => {
            Check::warn("devices", "no paired phone; run `openmosh-hook pair`")
        }
        Ok(d) => {
            let stale = d.iter().filter(|x| x.endpoint_stale).count();
            let silent = d.iter().filter(|x| x.endpoint.is_empty()).count();
            let mut notes = Vec::new();
            if stale > 0 {
                notes.push(format!(
                    "{stale} with a stale push endpoint (open the app to refresh)"
                ));
            }
            if silent > 0 {
                notes.push(format!(
                    "{silent} without notifications (Enable notifications on the host in the app)"
                ));
            }
            if notes.is_empty() {
                Check::ok("devices", format!("{} paired", d.len()))
            } else {
                Check::warn(
                    "devices",
                    format!("{} paired, {}", d.len(), notes.join(", ")),
                )
            }
        }
    }
}

pub fn check_keymaps(maps: &[Keymap]) -> Check {
    let unverified: Vec<&str> = maps
        .iter()
        .filter(|k| !k.verified)
        .map(|k| k.agent.as_str())
        .collect();
    if unverified.is_empty() {
        Check::ok("keymaps", format!("{} agent keymap(s)", maps.len()))
    } else {
        Check::warn(
            "keymaps",
            format!("not yet field-verified: {}", unverified.join(", ")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agents::domain::fakes::FakeRuntime;
    use crate::features::agents::domain::Runtime;
    use crate::features::doctor::domain::Status;
    use crate::features::pairing::domain::fakes::{FakeTailnet, MemRegistry};
    use crate::features::pairing::domain::{Device, TailnetStatus};

    #[test]
    fn tailscale_states() {
        let mut t = FakeTailnet::running("me", &[]);
        assert_eq!(check_tailscale(&t).status, Status::Ok);
        t.status = TailnetStatus {
            running: false,
            ..Default::default()
        };
        assert_eq!(check_tailscale(&t).status, Status::Fail);
        t.missing = true;
        assert_eq!(check_tailscale(&t).status, Status::Fail);
    }

    #[test]
    fn runtimes_need_at_least_one() {
        let h = FakeRuntime::herdr_with_agents(0);
        let t = FakeRuntime::unavailable(Runtime::Tmux);
        let both: [&dyn AgentRuntime; 2] = [&h, &t];
        let c = check_runtimes(&both);
        assert_eq!(
            c.iter().map(|c| c.status).collect::<Vec<_>>(),
            vec![Status::Ok, Status::Warn]
        );
        let none: [&dyn AgentRuntime; 1] = [&t];
        assert_eq!(check_runtimes(&none).last().unwrap().status, Status::Fail);
    }

    #[test]
    fn devices_and_keymaps() {
        assert_eq!(check_devices(&MemRegistry::default()).status, Status::Warn);
        let d = Device {
            id: "a".into(),
            name: "n".into(),
            ppk: String::new(),
            endpoint: String::new(),
            paired_at: 0,
            endpoint_stale: true,
            ssh_key: None,
            peer_ips: Vec::new(),
        };
        assert_eq!(
            check_devices(&MemRegistry::with(vec![d.clone()])).status,
            Status::Warn
        );
        let quiet = Device {
            endpoint_stale: false,
            ..d
        };
        assert_eq!(
            check_devices(&MemRegistry::with(vec![quiet])).detail,
            "1 paired, 1 without notifications (Enable notifications on the host in the app)"
        );
        let k = Keymap {
            agent: "claude".into(),
            version: 1,
            verified: false,
            allow: vec![],
            deny: vec![],
            submit: vec![],
        };
        assert_eq!(check_keymaps(&[k]).detail, "not yet field-verified: claude");
    }
}
