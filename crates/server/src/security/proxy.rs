//! Reverse-proxy client-IP extraction.
//!
//! `X-Forwarded-For` is only meaningful when the immediate peer is a trusted
//! proxy: otherwise any client could forge its own address and dodge rate
//! limiting. When the peer is trusted, the header is walked from right to
//! left, skipping further trusted proxies; the first untrusted address is the
//! real client. A forged prefix added by an attacker is ignored because the
//! rightmost untrusted hop is the address the last trusted proxy saw.

use std::net::IpAddr;

use ipnet::IpNet;

/// Resolves the real client address from the peer address and an optional
/// `X-Forwarded-For` header.
pub fn client_ip(peer: IpAddr, trusted: &[IpNet], forwarded_for: Option<&str>) -> IpAddr {
    if !is_trusted(peer, trusted) {
        return peer;
    }
    let Some(header) = forwarded_for else {
        return peer;
    };
    for entry in header.rsplit(',') {
        let candidate = entry.trim();
        if candidate.is_empty() {
            continue;
        }
        match candidate.parse::<IpAddr>() {
            Ok(ip) => {
                if !is_trusted(ip, trusted) {
                    return ip;
                }
            }
            Err(_) => {
                // Malformed hop: stop trusting the chain and fall back to the
                // peer rather than guessing.
                return peer;
            }
        }
    }
    peer
}

/// Whether `ip` falls inside any trusted network.
pub fn is_trusted(ip: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| match (net, ip) {
        (IpNet::V4(net), IpAddr::V4(ip)) => net.contains(&ip),
        (IpNet::V6(net), IpAddr::V6(ip)) => net.contains(&ip),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn nets(entries: &[&str]) -> Vec<IpNet> {
        entries
            .iter()
            .map(|e| IpNet::from_str(e).unwrap())
            .collect()
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_for() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(ip("8.8.8.8"), &trusted, Some("1.2.3.4"));
        assert_eq!(resolved, ip("8.8.8.8"));
    }

    #[test]
    fn trusted_peer_uses_forwarded_for() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(ip("10.0.0.5"), &trusted, Some("203.0.113.7"));
        assert_eq!(resolved, ip("203.0.113.7"));
    }

    #[test]
    fn forwarded_chain_skips_trusted_hops_from_the_right() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(
            ip("10.0.0.5"),
            &trusted,
            Some("6.6.6.6, 203.0.113.7, 10.0.0.9"),
        );
        assert_eq!(resolved, ip("203.0.113.7"));
    }

    #[test]
    fn forged_prefix_is_ignored() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(
            ip("10.0.0.5"),
            &trusted,
            Some("1.1.1.1, 198.51.100.4, 10.0.0.9"),
        );
        assert_eq!(resolved, ip("198.51.100.4"));
    }

    #[test]
    fn all_trusted_chain_falls_back_to_peer() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(ip("10.0.0.5"), &trusted, Some("10.1.1.1, 10.2.2.2"));
        assert_eq!(resolved, ip("10.0.0.5"));
    }

    #[test]
    fn malformed_hop_falls_back_to_peer() {
        let trusted = nets(&["10.0.0.0/8"]);
        let resolved = client_ip(ip("10.0.0.5"), &trusted, Some("not-an-ip"));
        assert_eq!(resolved, ip("10.0.0.5"));
    }

    #[test]
    fn no_header_uses_peer() {
        let trusted = nets(&["10.0.0.0/8"]);
        assert_eq!(client_ip(ip("10.0.0.5"), &trusted, None), ip("10.0.0.5"));
    }
}
