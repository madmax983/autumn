use axum::extract::ConnectInfo;
use axum::http::Request;
use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxy {
    pub network: IpAddr,
    pub prefix_len: u8,
}

impl TrustedProxy {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        let (addr, prefix_len) = if let Some((addr, prefix)) = trimmed.split_once('/') {
            let addr = addr.trim().parse::<IpAddr>().ok()?;
            let prefix_len = prefix.trim().parse::<u8>().ok()?;
            (addr, prefix_len)
        } else {
            let addr = trimmed.parse::<IpAddr>().ok()?;
            let prefix_len = match addr {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            };
            (addr, prefix_len)
        };

        let max_prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        (prefix_len <= max_prefix).then_some(Self {
            network: addr,
            prefix_len,
        })
    }

    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        if self.prefix_len == 0 {
            return matches!(
                (self.network, ip),
                (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
            );
        }

        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                let shift = 32_u8.saturating_sub(self.prefix_len);
                (u32::from(network) >> shift) == (u32::from(candidate) >> shift)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                let shift = 128_u8.saturating_sub(self.prefix_len);
                (u128::from(network) >> shift) == (u128::from(candidate) >> shift)
            }
            (IpAddr::V4(_), IpAddr::V6(_)) | (IpAddr::V6(_), IpAddr::V4(_)) => false,
        }
    }
}

/// Extract the originating client IP from a request, honoring the trusted-proxy policy.
pub fn extract_client_ip<B>(
    req: &Request<B>,
    trust_forwarded_headers: bool,
    trusted_proxies: &[TrustedProxy],
    trusted_proxies_configured: bool,
) -> Option<IpAddr> {
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
        .or_else(|| req.extensions().get::<SocketAddr>().map(SocketAddr::ip));

    if trust_forwarded_headers {
        let allowed = if trusted_proxies_configured {
            peer_ip.is_some_and(|ip| is_trusted_proxy(ip, trusted_proxies))
        } else {
            true
        };

        if allowed {
            let all_xff: Vec<&str> = req
                .headers()
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();

            if !all_xff.is_empty() {
                let joined = all_xff.join(", ");
                if let Some(xff_ip) = client_ip_from_x_forwarded_for(
                    &joined,
                    peer_ip,
                    trusted_proxies,
                    trusted_proxies_configured,
                ) {
                    return Some(xff_ip);
                }
            }

            if let Some(real_ip) = req
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<IpAddr>().ok())
            {
                return Some(real_ip);
            }
        }
    }

    peer_ip
}

fn client_ip_from_x_forwarded_for(
    header: &str,
    peer_ip: Option<IpAddr>,
    trusted_proxies: &[TrustedProxy],
    trusted_proxies_configured: bool,
) -> Option<IpAddr> {
    if !trusted_proxies_configured {
        let mut entries = header.rsplit(',').map(str::trim).filter(|s| !s.is_empty());
        let last = entries.next()?;

        if let Ok(last_ip) = last.parse::<IpAddr>() {
            if peer_ip.is_some_and(|peer_ip| last_ip == peer_ip)
                && let Some(prev) = entries.next()
                && let Ok(prev_ip) = prev.parse::<IpAddr>()
            {
                return Some(prev_ip);
            }
            return Some(last_ip);
        }
        return None;
    }

    for entry in header.rsplit(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(ip) = entry.parse::<IpAddr>()
            && !is_trusted_proxy(ip, trusted_proxies)
        {
            return Some(ip);
        }
    }

    None
}

fn is_trusted_proxy(ip: IpAddr, trusted_proxies: &[TrustedProxy]) -> bool {
    trusted_proxies
        .iter()
        .any(|trusted_proxy| trusted_proxy.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use std::net::{Ipv4Addr, SocketAddr};

    #[test]
    fn test_trusted_proxy_parse_valid_ipv4_no_cidr() {
        let p = TrustedProxy::parse("192.168.1.1").unwrap();
        assert_eq!(p.network, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(p.prefix_len, 32);
    }

    #[test]
    fn test_trusted_proxy_parse_valid_ipv4_with_cidr() {
        let p = TrustedProxy::parse("10.0.0.0/8").unwrap();
        assert_eq!(p.network, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)));
        assert_eq!(p.prefix_len, 8);
    }

    #[test]
    fn test_trusted_proxy_parse_valid_ipv6_no_cidr() {
        let p = TrustedProxy::parse("2001:db8::1").unwrap();
        assert_eq!(p.network, "2001:db8::1".parse::<IpAddr>().unwrap());
        assert_eq!(p.prefix_len, 128);
    }

    #[test]
    fn test_trusted_proxy_parse_valid_ipv6_with_cidr() {
        let p = TrustedProxy::parse("2001:db8::/32").unwrap();
        assert_eq!(p.network, "2001:db8::".parse::<IpAddr>().unwrap());
        assert_eq!(p.prefix_len, 32);
    }

    #[test]
    fn test_trusted_proxy_parse_invalid() {
        assert!(TrustedProxy::parse("").is_none());
        assert!(TrustedProxy::parse("   ").is_none());
        assert!(TrustedProxy::parse("invalid-ip").is_none());
        assert!(TrustedProxy::parse("192.168.1.1/33").is_none()); // Invalid IPv4 CIDR
        assert!(TrustedProxy::parse("2001:db8::1/129").is_none()); // Invalid IPv6 CIDR
        assert!(TrustedProxy::parse("192.168.1.1/invalid").is_none());
        assert!(TrustedProxy::parse("192.168.1.1/").is_none());
    }

    #[test]
    fn test_trusted_proxy_contains_ipv4() {
        let p = TrustedProxy::parse("192.168.1.0/24").unwrap();
        assert!(p.contains("192.168.1.1".parse().unwrap()));
        assert!(p.contains("192.168.1.255".parse().unwrap()));
        assert!(!p.contains("192.168.2.1".parse().unwrap()));

        let p_exact = TrustedProxy::parse("10.0.0.1").unwrap();
        assert!(p_exact.contains("10.0.0.1".parse().unwrap()));
        assert!(!p_exact.contains("10.0.0.2".parse().unwrap()));

        let p_all = TrustedProxy::parse("0.0.0.0/0").unwrap();
        assert!(p_all.contains("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_trusted_proxy_contains_ipv6() {
        let p = TrustedProxy::parse("2001:db8::/32").unwrap();
        assert!(p.contains("2001:db8::1".parse().unwrap()));
        assert!(p.contains("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()));
        assert!(!p.contains("2001:db9::1".parse().unwrap()));

        let p_exact = TrustedProxy::parse("2001:db8::1").unwrap();
        assert!(p_exact.contains("2001:db8::1".parse().unwrap()));
        assert!(!p_exact.contains("2001:db8::2".parse().unwrap()));

        let p_all = TrustedProxy::parse("::/0").unwrap();
        assert!(p_all.contains("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn test_trusted_proxy_contains_mismatched_families() {
        let p_v4 = TrustedProxy::parse("192.168.1.0/24").unwrap();
        let p_v6 = TrustedProxy::parse("2001:db8::/32").unwrap();

        assert!(!p_v4.contains("2001:db8::1".parse().unwrap()));
        assert!(!p_v6.contains("192.168.1.1".parse().unwrap()));

        let p_v4_all = TrustedProxy::parse("0.0.0.0/0").unwrap();
        let p_v6_all = TrustedProxy::parse("::/0").unwrap();

        assert!(!p_v4_all.contains("2001:db8::1".parse().unwrap()));
        assert!(!p_v6_all.contains("192.168.1.1".parse().unwrap()));
    }

    fn build_req_with_peer(ip: &str) -> Request<()> {
        let mut req = Request::new(());
        let addr = SocketAddr::new(ip.parse().unwrap(), 8080);
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    #[test]
    fn test_extract_client_ip_no_trust_forwarded_headers() {
        let req = build_req_with_peer("192.168.1.1");
        let ip = extract_client_ip(&req, false, &[], false);
        assert_eq!(ip.unwrap(), "192.168.1.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_trust_but_no_xff_or_real_ip() {
        let req = build_req_with_peer("10.0.0.1");
        let trusted_proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        assert_eq!(ip.unwrap(), "10.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_xff_trusted_proxies_configured() {
        let mut req = build_req_with_peer("10.0.0.1"); // Trusted peer
        req.headers_mut().insert(
            "x-forwarded-for",
            "203.0.113.1, 192.168.1.1".parse().unwrap(),
        );

        // Trust 10.0.0.0/8 and 192.168.1.0/24
        let trusted_proxies = vec![
            TrustedProxy::parse("10.0.0.0/8").unwrap(),
            TrustedProxy::parse("192.168.1.0/24").unwrap(),
        ];

        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        // It walks from right: 192.168.1.1 (trusted), then 203.0.113.1 (untrusted) -> returns 203.0.113.1
        assert_eq!(ip.unwrap(), "203.0.113.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_xff_untrusted_peer_ignored() {
        let mut req = build_req_with_peer("203.0.113.2"); // Untrusted peer
        req.headers_mut()
            .insert("x-forwarded-for", "1.1.1.1".parse().unwrap());

        let trusted_proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);

        // Peer is untrusted, so XFF is ignored, returns peer
        assert_eq!(ip.unwrap(), "203.0.113.2".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_xff_not_configured() {
        // When trusted_proxies_configured is false, any peer is trusted.
        // It expects the last proxy to be the peer, and looks for prev.
        let mut req = build_req_with_peer("203.0.113.1");
        req.headers_mut()
            .insert("x-forwarded-for", "1.1.1.1, 203.0.113.1".parse().unwrap());

        let ip = extract_client_ip(&req, true, &[], false);
        // last == peer, returns prev
        assert_eq!(ip.unwrap(), "1.1.1.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_xff_not_configured_last_not_peer() {
        let mut req = build_req_with_peer("10.0.0.1");
        req.headers_mut()
            .insert("x-forwarded-for", "1.1.1.1, 203.0.113.1".parse().unwrap());

        let ip = extract_client_ip(&req, true, &[], false);
        // last != peer, returns last
        assert_eq!(ip.unwrap(), "203.0.113.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_extract_client_ip_fallback_to_x_real_ip() {
        let mut req = build_req_with_peer("10.0.0.1");
        req.headers_mut()
            .insert("x-real-ip", "1.1.1.1".parse().unwrap());

        let trusted_proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        assert_eq!(ip.unwrap(), "1.1.1.1".parse::<IpAddr>().unwrap());
    }
}
