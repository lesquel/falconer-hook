//! Push endpoint validation. The URL is parsed with a WHATWG URL parser
//! (the same rules the HTTP client applies), then:
//! - no userinfo, no port 0;
//! - the host is never loopback (`127/8`, `::1`, `localhost`), private
//!   (`10/8`, `172.16/12`, `192.168/16`, `fc00::/7`), link-local
//!   (`169.254/16`, `fe80::/10`), unspecified (`0/8`, `::`), multicast or
//!   broadcast; Tailscale addresses (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`)
//!   are allowed. The HTTP client applies the same rule to every address a
//!   name resolves to ([`forbidden_ip`]), so DNS cannot point a public name
//!   at the LAN or at the host itself;
//! - `https://` to any other host; `http://` only to a Tailscale address
//!   (`100.64.0.0/10` or a name ending exactly in `.ts.net`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;
use url::{Host, Url};

use crate::shared::domain::net::{is_tailnet_ipv4, is_tailnet_ipv6};

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum EndpointError {
    #[error("endpoint must be an https:// URL (http:// only for tailnet hosts)")]
    Scheme,
    #[error("endpoint URL is malformed")]
    Malformed,
    #[error(
        "endpoint host is not allowed (loopback, private, link-local, unspecified or multicast)"
    )]
    Host,
}

fn forbidden_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.octets()[0] == 0
        || ip.is_multicast()
        || ip.is_broadcast()
}

fn forbidden_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return forbidden_v4(v4);
    }
    let link_local = (ip.segments()[0] & 0xffc0) == 0xfe80;
    let unique_local = (ip.segments()[0] & 0xfe00) == 0xfc00 && !is_tailnet_ipv6(ip);
    ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || link_local || unique_local
}

/// An address a push must never be sent to (see the module rules).
pub fn forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => forbidden_v4(v4),
        IpAddr::V6(v6) => forbidden_v6(v6),
    }
}

fn valid_dns_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('-')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// A Web Push endpoint: everything [`validate_endpoint`] requires, and always
/// `https://` (the VAPID token and the subscription would otherwise travel in
/// clear, even to a tailnet host).
pub fn validate_webpush_endpoint(raw: &str) -> Result<(), EndpointError> {
    validate_endpoint(raw)?;
    if !raw.to_ascii_lowercase().starts_with("https://") {
        return Err(EndpointError::Scheme);
    }
    Ok(())
}

pub fn validate_endpoint(raw: &str) -> Result<(), EndpointError> {
    // The parser silently drops tabs/newlines and treats `\` as `/`; refuse
    // such input instead of validating something other than what was given.
    if raw.is_empty()
        || raw.len() > 2048
        || raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(EndpointError::Malformed);
    }
    let (scheme, _) = raw.split_once("://").ok_or(EndpointError::Malformed)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(EndpointError::Scheme);
    }
    let url = Url::parse(raw).map_err(|_| EndpointError::Malformed)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(EndpointError::Malformed);
    }
    if url.port() == Some(0) {
        return Err(EndpointError::Malformed);
    }
    let tailnet = match url.host().ok_or(EndpointError::Malformed)? {
        Host::Ipv4(ip) => {
            if forbidden_v4(ip) {
                return Err(EndpointError::Host);
            }
            is_tailnet_ipv4(ip)
        }
        Host::Ipv6(ip) => {
            if forbidden_v6(ip) {
                return Err(EndpointError::Host);
            }
            false
        }
        Host::Domain(name) => {
            // Already lowercased and IDNA-mapped by the parser.
            let name = name.strip_suffix('.').unwrap_or(name);
            if !valid_dns_name(name) {
                return Err(EndpointError::Malformed);
            }
            if name == "localhost" || name.ends_with(".localhost") {
                return Err(EndpointError::Host);
            }
            name.strip_suffix(".ts.net")
                .is_some_and(|label| !label.is_empty())
        }
    };
    if scheme == "http" && !tailnet {
        return Err(EndpointError::Scheme);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_to_any_host_is_allowed() {
        for u in [
            "https://ntfy.sh/upAbC123?up=1",
            "https://push.example.com:8443/x",
            "HTTPS://ntfy.sh/x",
            "https://[2001:db8::1]:443/x",
        ] {
            assert_eq!(validate_endpoint(u), Ok(()), "{u}");
        }
    }

    #[test]
    fn http_only_for_tailnet() {
        assert_eq!(validate_endpoint("http://100.101.1.2:2586/up"), Ok(()));
        assert_eq!(
            validate_endpoint("http://ntfy.tailnet-example.ts.net/up"),
            Ok(())
        );
        assert_eq!(
            validate_endpoint("http://ntfy.sh/up"),
            Err(EndpointError::Scheme)
        );
        assert_eq!(
            validate_endpoint("http://192.168.1.2/up"),
            Err(EndpointError::Host)
        );
        assert_eq!(
            validate_endpoint("http://100.200.1.2/up"),
            Err(EndpointError::Scheme)
        );
        assert_eq!(
            validate_endpoint("http://evilts.net/up"),
            Err(EndpointError::Scheme)
        );
    }

    #[test]
    fn local_and_special_hosts_are_rejected() {
        for u in [
            "https://127.0.0.1/x",
            "https://127.5.5.5:8443/x",
            "https://0x7f.1/x",
            "https://2130706433/x",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://localhost/x",
            "https://LocalHost./x",
            "https://api.localhost/x",
            "https://169.254.169.254/latest",
            "https://[fe80::1]/x",
            "https://[febf::1]/x",
            "https://0.0.0.0/x",
            "https://[::]/x",
            "https://224.0.0.1/x",
            "https://[ff02::1]/x",
            "https://255.255.255.255/x",
            "http://localhost.ts.net.evil/x",
            "https://10.0.0.5/x",
            "https://172.16.0.1/x",
            "https://172.31.255.255/x",
            "https://192.168.1.10:8443/x",
            "https://0.1.2.3/x",
            "https://[fc00::1]/x",
            "https://[fd12:3456::1]/x",
            "https://[::ffff:10.1.1.1]/x",
        ] {
            assert!(validate_endpoint(u).is_err(), "{u}");
        }
        assert_eq!(
            validate_endpoint("https://127.0.0.1/x"),
            Err(EndpointError::Host)
        );
        // Neighbours of the forbidden ranges stay valid.
        for u in [
            "https://[fec0::1]/x",
            "https://169.255.0.1/x",
            "https://128.0.0.1/x",
            "https://172.32.0.1/x",
            "https://100.101.1.2/x",
            "https://[fd7a:115c:a1e0::1]/x",
        ] {
            assert_eq!(validate_endpoint(u), Ok(()), "{u}");
        }
    }

    #[test]
    fn ts_net_suffix_is_exact_and_case_insensitive() {
        assert_eq!(
            validate_endpoint("http://Ntfy.Tail08C84D.TS.NET/up"),
            Ok(())
        );
        assert_eq!(validate_endpoint("http://ntfy.tail.ts.net./up"), Ok(()));
        for u in [
            "http://ts.net/up",
            "http://.ts.net/up",
            "http://ntfy.ts.net.evil.com/up",
            "http://ntfyts.net/up",
            "http://100.64.0.1.nip.io/up",
        ] {
            assert!(validate_endpoint(u).is_err(), "{u}");
        }
    }

    #[test]
    fn malformed_and_other_schemes_are_rejected() {
        assert_eq!(
            validate_endpoint("ftp://ntfy.sh"),
            Err(EndpointError::Scheme)
        );
        assert_eq!(
            validate_endpoint("file:///etc/passwd"),
            Err(EndpointError::Scheme)
        );
        for u in [
            "",
            "ntfy.sh/up",
            "https://",
            "https://user:pw@ntfy.sh/up",
            "https://ntfy.sh:0/x",
            "https://ntfy.sh:99999/x",
            "https://nt fy.sh/x",
            "https://ntfy.sh/x\n",
            "https://ntfy_sh/x",
            "https://:pw@ntfy.sh/up",
            "https://user@ntfy.sh/up",
            "https://[::1/x",
            "https://[zz]/x",
            "https://[::1]x/y",
            "https:\\\\ntfy.sh\\x",
            "https:ntfy.sh/x",
        ] {
            assert_eq!(validate_endpoint(u), Err(EndpointError::Malformed), "{u:?}");
        }
    }
}
