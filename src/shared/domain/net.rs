//! Tailnet address rules.

use std::net::{IpAddr, Ipv4Addr};

/// Tailscale CGNAT range `100.64.0.0/10`.
pub fn is_tailnet_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0xC0) == 64
}

/// Accepts IPv4 and IPv4-mapped IPv6 inside `100.64.0.0/10`.
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_tailnet_ipv4(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(is_tailnet_ipv4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgnat_range_bounds() {
        assert!(is_tailnet_ipv4("100.64.0.0".parse().unwrap()));
        assert!(is_tailnet_ipv4("100.88.77.66".parse().unwrap()));
        assert!(is_tailnet_ipv4("100.127.255.255".parse().unwrap()));
        assert!(!is_tailnet_ipv4("100.63.255.255".parse().unwrap()));
        assert!(!is_tailnet_ipv4("100.128.0.0".parse().unwrap()));
        assert!(!is_tailnet_ipv4("192.168.1.10".parse().unwrap()));
    }

    #[test]
    fn mapped_ipv6_is_unwrapped() {
        assert!(is_tailnet_ip("::ffff:100.100.1.1".parse().unwrap()));
        assert!(!is_tailnet_ip("fd7a:115c:a1e0::1".parse().unwrap()));
    }
}
