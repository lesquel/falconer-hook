//! Tailnet address rules.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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

/// Tailscale's IPv6 ULA range `fd7a:115c:a1e0::/48`.
pub fn is_tailnet_ipv6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
}

/// A Tailscale node address, IPv4 (`100.64.0.0/10`) or IPv6 (`fd7a:115c:a1e0::/48`).
pub fn is_tailnet_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_none() => is_tailnet_ipv6(v6),
        _ => is_tailnet_ip(ip),
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

    #[test]
    fn tailnet_addresses_cover_both_families() {
        assert!(is_tailnet_addr("fd7a:115c:a1e0:ab12::1".parse().unwrap()));
        assert!(is_tailnet_addr("100.101.1.2".parse().unwrap()));
        assert!(!is_tailnet_addr("fd7a:115c:a1e1::1".parse().unwrap()));
        assert!(!is_tailnet_addr("fe80::1".parse().unwrap()));
    }
}
