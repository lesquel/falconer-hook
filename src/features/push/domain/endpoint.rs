//! Push endpoint validation: `https://` to any host; `http://` only to a
//! Tailscale address (`100.64.0.0/10` or a `*.ts.net` name).

use std::net::Ipv4Addr;
use thiserror::Error;

use crate::shared::domain::net::is_tailnet_ipv4;

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum EndpointError {
    #[error("endpoint must be an https:// URL (http:// only for tailnet hosts)")]
    Scheme,
    #[error("endpoint URL is malformed")]
    Malformed,
}

pub fn validate_endpoint(url: &str) -> Result<(), EndpointError> {
    if url.is_empty()
        || url.len() > 2048
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(EndpointError::Malformed);
    }
    let (scheme, rest) = url.split_once("://").ok_or(EndpointError::Malformed)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(EndpointError::Scheme);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains('@') {
        return Err(EndpointError::Malformed);
    }
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let close = after.find(']').ok_or(EndpointError::Malformed)?;
        let host = &after[..close];
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(EndpointError::Malformed);
        }
        (host, after[close + 1..].strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if let Some(p) = port {
        match p.parse::<u16>() {
            Ok(n) if n > 0 => {}
            _ => return Err(EndpointError::Malformed),
        }
    }
    if !authority.starts_with('[')
        && (host.is_empty()
            || host.starts_with('.')
            || host.starts_with('-')
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'))
    {
        return Err(EndpointError::Malformed);
    }
    if scheme == "http" {
        let lower = host.to_ascii_lowercase();
        let tailnet_ip = lower.parse::<Ipv4Addr>().is_ok_and(is_tailnet_ipv4);
        let ts_name =
            lower.len() > ".ts.net".len() && lower.trim_end_matches('.').ends_with(".ts.net");
        if !(tailnet_ip || ts_name) {
            return Err(EndpointError::Scheme);
        }
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
            Err(EndpointError::Scheme)
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
        ] {
            assert_eq!(validate_endpoint(u), Err(EndpointError::Malformed), "{u:?}");
        }
    }
}
