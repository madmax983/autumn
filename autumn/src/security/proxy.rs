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
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::str::FromStr;

    #[test]
    fn test_trusted_proxy_parse() {
        // Valid v4
        let p = TrustedProxy::parse("192.168.1.1/24").unwrap();
        assert_eq!(p.network, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(p.prefix_len, 24);

        let p = TrustedProxy::parse("10.0.0.1").unwrap();
        assert_eq!(p.network, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(p.prefix_len, 32);

        // Valid v6
        let p = TrustedProxy::parse("2001:db8::1/64").unwrap();
        assert_eq!(
            p.network,
            IpAddr::V6(Ipv6Addr::from_str("2001:db8::1").unwrap())
        );
        assert_eq!(p.prefix_len, 64);

        let p = TrustedProxy::parse("::1").unwrap();
        assert_eq!(p.network, IpAddr::V6(Ipv6Addr::from_str("::1").unwrap()));
        assert_eq!(p.prefix_len, 128);

        // Invalid
        assert!(TrustedProxy::parse("").is_none());
        assert!(TrustedProxy::parse("invalid").is_none());
        assert!(TrustedProxy::parse("192.168.1.1/invalid").is_none());
        assert!(TrustedProxy::parse("192.168.1.1/33").is_none());
        assert!(TrustedProxy::parse("::1/129").is_none());
    }

    #[test]
    fn test_trusted_proxy_contains() {
        let p_v4 = TrustedProxy::parse("192.168.1.0/24").unwrap();
        assert!(p_v4.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))));
        assert!(!p_v4.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 1))));
        assert!(!p_v4.contains(IpAddr::V6(Ipv6Addr::from_str("::1").unwrap())));

        let p_v6 = TrustedProxy::parse("2001:db8::/64").unwrap();
        assert!(p_v6.contains(IpAddr::V6(Ipv6Addr::from_str("2001:db8::1").unwrap())));
        assert!(!p_v6.contains(IpAddr::V6(Ipv6Addr::from_str("2001:db9::1").unwrap())));
        assert!(!p_v6.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));

        let p_zero_v4 = TrustedProxy::parse("0.0.0.0/0").unwrap();
        assert!(p_zero_v4.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(!p_zero_v4.contains(IpAddr::V6(Ipv6Addr::from_str("::1").unwrap())));

        let p_zero_v6 = TrustedProxy::parse("::/0").unwrap();
        assert!(p_zero_v6.contains(IpAddr::V6(Ipv6Addr::from_str("2001:db8::1").unwrap())));
        assert!(!p_zero_v6.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_is_trusted_proxy() {
        let proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];
        assert!(is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)),
            &proxies
        ));
        assert!(!is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &proxies
        ));
    }

    #[test]
    fn test_client_ip_from_x_forwarded_for() {
        let peer_ip = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        let trusted_proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];

        // Unconfigured proxies (trust all)
        assert_eq!(
            client_ip_from_x_forwarded_for("1.2.3.4, 5.6.7.8, 10.0.0.1", peer_ip, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8))) // Uses the one before the peer
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("1.2.3.4, 5.6.7.8", peer_ip, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8))) // Doesn't match peer, uses last valid
        );

        // Configured proxies
        assert_eq!(
            client_ip_from_x_forwarded_for("1.2.3.4, 10.0.0.2", peer_ip, &trusted_proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))) // 10.0.0.2 is trusted, so it looks at the previous one
        );

        assert_eq!(
            client_ip_from_x_forwarded_for(
                "1.2.3.4, 5.6.7.8, 10.0.0.2",
                peer_ip,
                &trusted_proxies,
                true
            ),
            Some(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8))) // 10.0.0.2 is trusted, 5.6.7.8 is not, stops there
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("invalid", peer_ip, &[], false),
            None
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("invalid", peer_ip, &trusted_proxies, true),
            None
        );
    }

    #[test]
    fn test_extract_client_ip() {
        let mut req = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            8080,
        )));
        req.headers_mut()
            .insert("x-forwarded-for", "1.2.3.4, 10.0.0.2".parse().unwrap());
        req.headers_mut()
            .insert("x-real-ip", "5.6.7.8".parse().unwrap());

        let trusted_proxies = vec![TrustedProxy::parse("10.0.0.0/8").unwrap()];

        // trust_forwarded_headers = false
        assert_eq!(
            extract_client_ip(&req, false, &trusted_proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );

        // trust_forwarded_headers = true, but peer not trusted
        let untrusted_proxies = vec![TrustedProxy::parse("192.168.1.0/24").unwrap()];
        assert_eq!(
            extract_client_ip(&req, true, &untrusted_proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );

        // trust_forwarded_headers = true, peer is trusted
        assert_eq!(
            extract_client_ip(&req, true, &trusted_proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)))
        );

        // test fallback to x-real-ip
        let mut req2 = Request::builder().body(()).unwrap();
        req2.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            8080,
        )));
        req2.headers_mut()
            .insert("x-real-ip", "5.6.7.8".parse().unwrap());

        assert_eq!(
            extract_client_ip(&req2, true, &trusted_proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)))
        );
    }
}
