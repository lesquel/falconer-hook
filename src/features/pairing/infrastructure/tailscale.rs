//! Read-only Tailscale CLI adapter: `tailscale status --json`,
//! `tailscale ip -4`, `tailscale whois --json <ip>`.

use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::features::pairing::domain::{Peer, Tailnet, TailnetError, TailnetStatus};
use crate::shared::domain::command::{CommandError, CommandRunner};

pub struct TailscaleCli<'a> {
    pub runner: &'a dyn CommandRunner,
    pub program: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<SelfNode>,
    #[serde(default)]
    user: HashMap<String, UserProfile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SelfNode {
    #[serde(rename = "UserID")]
    user_id: u64,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserProfile {
    login_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Whois {
    node: Option<WhoisNode>,
    user_profile: Option<UserProfile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WhoisNode {
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    name: String,
    /// CIDR strings (`100.101.1.2/32`, `fd7a:…/128`).
    #[serde(default)]
    addresses: Vec<String>,
}

/// `LoginName` Tailscale reports for nodes owned by tags instead of a user.
pub const TAGGED_LOGIN: &str = "tagged-devices";

fn has_tags(tags: &Option<Vec<String>>) -> bool {
    tags.as_ref().is_some_and(|t| !t.is_empty())
}

pub fn parse_status(stdout: &str) -> Result<TailnetStatus, TailnetError> {
    let s: Status = serde_json::from_str(stdout).map_err(|_| TailnetError::Failed)?;
    let login_name = s
        .self_node
        .as_ref()
        .and_then(|n| s.user.get(&n.user_id.to_string()))
        .map(|u| u.login_name.clone());
    let dns_name = s
        .self_node
        .as_ref()
        .map(|n| n.dns_name.trim_end_matches('.').to_string())
        .filter(|d| !d.is_empty());
    let tagged = s.self_node.as_ref().is_some_and(|n| has_tags(&n.tags))
        || login_name.as_deref() == Some(TAGGED_LOGIN);
    Ok(TailnetStatus {
        running: s.backend_state == "Running",
        login_name,
        dns_name,
        tagged,
    })
}

/// The user-owned node of a whois result. Tagged nodes and results without a
/// `Node` or `UserProfile` have none, so they can never match the host.
pub fn parse_whois(stdout: &str) -> Option<Peer> {
    let w: Whois = serde_json::from_str(stdout).ok()?;
    let node = w.node?;
    if has_tags(&node.tags) {
        return None;
    }
    let login = w
        .user_profile
        .map(|u| u.login_name)
        .filter(|l| !l.is_empty() && l != TAGGED_LOGIN)?;
    let addresses = node
        .addresses
        .iter()
        .filter_map(|a| a.split('/').next()?.parse::<IpAddr>().ok())
        .collect();
    let name = node.name.trim_end_matches('.');
    Some(Peer {
        login,
        node: (!name.is_empty()).then(|| name.to_string()),
        addresses,
    })
}

impl TailscaleCli<'_> {
    fn run(
        &self,
        args: &[&str],
    ) -> Result<crate::shared::domain::command::CommandOutput, TailnetError> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.runner.run(&self.program, &args).map_err(|e| match e {
            CommandError::NotFound(_) => TailnetError::Missing,
            _ => TailnetError::Failed,
        })
    }
}

impl Tailnet for TailscaleCli<'_> {
    fn status(&self) -> Result<TailnetStatus, TailnetError> {
        let out = self.run(&["status", "--json"])?;
        // `tailscale status` exits non-zero when stopped but still prints JSON.
        parse_status(&out.stdout).or_else(|e| {
            if out.success() {
                Err(e)
            } else {
                Ok(TailnetStatus::default())
            }
        })
    }

    fn ipv4(&self) -> Result<Ipv4Addr, TailnetError> {
        let out = self.run(&["ip", "-4"])?;
        if !out.success() {
            return Err(TailnetError::Failed);
        }
        out.stdout
            .lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
            .ok_or(TailnetError::Failed)
    }

    fn whois(&self, ip: IpAddr) -> Result<Option<Peer>, TailnetError> {
        let out = self.run(&["whois", "--json", &ip.to_string()])?;
        if !out.success() {
            return Ok(None);
        }
        Ok(parse_whois(&out.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::domain::command::fakes::RecordingRunner;
    use crate::shared::domain::command::CommandOutput;

    const STATUS: &str = r#"{"Version":"1.104.2","BackendState":"Running","Self":{"ID":"n","UserID":7149192543815875,"HostName":"box","DNSName":"box.tailnet-example.ts.net.","TailscaleIPs":["100.88.77.66"]},"User":{"7149192543815875":{"ID":7149192543815875,"LoginName":"me@example.com","DisplayName":"Me"}}}"#;

    #[test]
    fn parses_status_shape() {
        let s = parse_status(STATUS).unwrap();
        assert!(s.running);
        assert_eq!(s.login_name.as_deref(), Some("me@example.com"));
        assert_eq!(s.dns_name.as_deref(), Some("box.tailnet-example.ts.net"));
        let stopped = parse_status(r#"{"BackendState":"Stopped","Self":null,"User":null}"#);
        assert!(stopped.is_err() || !stopped.unwrap().running);
    }

    #[test]
    fn tagged_host_is_flagged() {
        let tagged = r#"{"BackendState":"Running","Self":{"UserID":1,"DNSName":"box.ts.net.","Tags":["tag:server"]},"User":{"1":{"LoginName":"tagged-devices"}}}"#;
        assert!(parse_status(tagged).unwrap().tagged);
        let by_login = r#"{"BackendState":"Running","Self":{"UserID":1},"User":{"1":{"LoginName":"tagged-devices"}}}"#;
        assert!(parse_status(by_login).unwrap().tagged);
        assert!(!parse_status(STATUS).unwrap().tagged);
    }

    #[test]
    fn tagged_or_incomplete_peers_have_no_login() {
        for bad in [
            r#"{"Node":{"Tags":["tag:phone"]},"UserProfile":{"LoginName":"me@example.com"}}"#,
            r#"{"Node":{},"UserProfile":{"LoginName":"tagged-devices"}}"#,
            r#"{"UserProfile":{"LoginName":"me@example.com"}}"#,
            r#"{"Node":{}}"#,
            r#"{"Node":{},"UserProfile":{"LoginName":""}}"#,
            "not json",
        ] {
            assert_eq!(parse_whois(bad), None, "{bad}");
        }
        assert_eq!(
            parse_whois(r#"{"Node":{"Tags":[]},"UserProfile":{"LoginName":"me@example.com"}}"#)
                .map(|p| p.login)
                .as_deref(),
            Some("me@example.com")
        );
    }

    #[test]
    fn whois_reports_the_node_name_and_addresses() {
        let p = parse_whois(r#"{"Node":{"Name":"pixel-8.tailnet-example.ts.net.","Addresses":["100.101.1.2/32","fd7a:115c:a1e0::1:2/128","junk"]},"UserProfile":{"LoginName":"me@example.com"}}"#).unwrap();
        assert_eq!(p.node.as_deref(), Some("pixel-8.tailnet-example.ts.net"));
        assert_eq!(
            p.addresses,
            vec![
                "100.101.1.2".parse::<IpAddr>().unwrap(),
                "fd7a:115c:a1e0::1:2".parse().unwrap()
            ]
        );
    }

    #[test]
    fn whois_and_ip_argv() {
        let r = RecordingRunner::default();
        r.reply_ok(r#"{"Node":{},"UserProfile":{"ID":1,"LoginName":"me@example.com","DisplayName":"Me"},"CapMap":{}}"#);
        r.reply_ok("100.88.77.66\n");
        r.reply(Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "no match".into(),
        }));
        let t = TailscaleCli {
            runner: &r,
            program: "tailscale".into(),
        };
        assert_eq!(
            t.whois("100.101.1.2".parse().unwrap())
                .unwrap()
                .map(|p| p.login)
                .as_deref(),
            Some("me@example.com")
        );
        assert_eq!(
            t.ipv4().unwrap(),
            "100.88.77.66".parse::<Ipv4Addr>().unwrap()
        );
        assert_eq!(t.whois("100.101.1.3".parse().unwrap()).unwrap(), None);
        assert_eq!(
            r.argv(),
            vec![
                vec!["tailscale", "whois", "--json", "100.101.1.2"],
                vec!["tailscale", "ip", "-4"],
                vec!["tailscale", "whois", "--json", "100.101.1.3"],
            ]
        );
    }
}
