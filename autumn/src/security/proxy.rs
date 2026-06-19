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
    use std::str::FromStr;

    #[test]
    fn test_trusted_proxy_parse() {
        assert_eq!(
            TrustedProxy::parse("192.168.1.1/24"),
            Some(TrustedProxy {
                network: IpAddr::from_str("192.168.1.1").unwrap(),
                prefix_len: 24,
            })
        );

        assert_eq!(
            TrustedProxy::parse("10.0.0.1"),
            Some(TrustedProxy {
                network: IpAddr::from_str("10.0.0.1").unwrap(),
                prefix_len: 32,
            })
        );

        assert_eq!(
            TrustedProxy::parse("2001:db8::1/64"),
            Some(TrustedProxy {
                network: IpAddr::from_str("2001:db8::1").unwrap(),
                prefix_len: 64,
            })
        );

        assert_eq!(
            TrustedProxy::parse("2001:db8::1"),
            Some(TrustedProxy {
                network: IpAddr::from_str("2001:db8::1").unwrap(),
                prefix_len: 128,
            })
        );

        assert_eq!(TrustedProxy::parse(""), None);
        assert_eq!(TrustedProxy::parse("invalid"), None);
        assert_eq!(TrustedProxy::parse("10.0.0.1/invalid"), None);
        assert_eq!(TrustedProxy::parse("10.0.0.1/33"), None); // > 32 for IPv4
        assert_eq!(TrustedProxy::parse("2001:db8::1/129"), None); // > 128 for IPv6
    }

    #[test]
    fn test_trusted_proxy_contains() {
        let proxy_ipv4 = TrustedProxy::parse("192.168.1.0/24").unwrap();
        assert!(proxy_ipv4.contains(IpAddr::from_str("192.168.1.100").unwrap()));
        assert!(!proxy_ipv4.contains(IpAddr::from_str("192.168.2.1").unwrap()));
        assert!(!proxy_ipv4.contains(IpAddr::from_str("2001:db8::1").unwrap()));

        let proxy_ipv6 = TrustedProxy::parse("2001:db8::/64").unwrap();
        assert!(proxy_ipv6.contains(IpAddr::from_str("2001:db8::1").unwrap()));
        assert!(!proxy_ipv6.contains(IpAddr::from_str("2001:db9::1").unwrap()));
        assert!(!proxy_ipv6.contains(IpAddr::from_str("192.168.1.1").unwrap()));

        let proxy_zero_prefix = TrustedProxy::parse("0.0.0.0/0").unwrap();
        assert!(proxy_zero_prefix.contains(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(!proxy_zero_prefix.contains(IpAddr::from_str("2001:db8::1").unwrap()));

        let proxy_v6_zero_prefix = TrustedProxy::parse("::/0").unwrap();
        assert!(proxy_v6_zero_prefix.contains(IpAddr::from_str("2001:db8::1").unwrap()));
        assert!(!proxy_v6_zero_prefix.contains(IpAddr::from_str("8.8.8.8").unwrap()));

        let proxy_exact = TrustedProxy::parse("10.0.0.1").unwrap();
        assert!(proxy_exact.contains(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!proxy_exact.contains(IpAddr::from_str("10.0.0.2").unwrap()));
    }

    #[test]
    fn test_client_ip_from_x_forwarded_for_no_proxies_configured() {
        let proxies = [];
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1", None, &proxies, false),
            Some(IpAddr::from_str("1.1.1.1").unwrap())
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("2.2.2.2, 1.1.1.1", None, &proxies, false),
            Some(IpAddr::from_str("1.1.1.1").unwrap())
        );

        let peer_ip = Some(IpAddr::from_str("1.1.1.1").unwrap());
        assert_eq!(
            client_ip_from_x_forwarded_for("2.2.2.2, 1.1.1.1", peer_ip, &proxies, false),
            Some(IpAddr::from_str("2.2.2.2").unwrap())
        );

        let peer_ip = Some(IpAddr::from_str("1.1.1.1").unwrap());
        assert_eq!(
            client_ip_from_x_forwarded_for("3.3.3.3, 2.2.2.2, 1.1.1.1", peer_ip, &proxies, false),
            Some(IpAddr::from_str("2.2.2.2").unwrap())
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("invalid", None, &proxies, false),
            None
        );
    }

    #[test]
    fn test_client_ip_from_x_forwarded_for_with_proxies_configured() {
        let proxies = [TrustedProxy::parse("10.0.0.0/8").unwrap()];

        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1", None, &proxies, true),
            Some(IpAddr::from_str("1.1.1.1").unwrap())
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("2.2.2.2, 10.0.0.1", None, &proxies, true),
            Some(IpAddr::from_str("2.2.2.2").unwrap())
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("10.0.0.2, 10.0.0.1", None, &proxies, true),
            None
        );

        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1, invalid, 10.0.0.1", None, &proxies, true),
            Some(IpAddr::from_str("1.1.1.1").unwrap()) // Unparsable IPs are skipped, returns first parsable untrusted IP
        );
    }

    #[test]
    fn test_extract_client_ip() {
        let proxies = [TrustedProxy::parse("10.0.0.1").unwrap()];

        // No headers, fallback to peer IP
        let mut req = Request::new(());
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::from_str("192.168.1.1").unwrap(),
            80,
        )));
        assert_eq!(
            extract_client_ip(&req, false, &proxies, false),
            Some(IpAddr::from_str("192.168.1.1").unwrap())
        );

        // Trust forwarded headers, but peer not trusted (proxies configured)
        let mut req = Request::new(());
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::from_str("192.168.1.1").unwrap(),
            80,
        )));
        req.headers_mut()
            .insert("x-forwarded-for", "8.8.8.8".parse().unwrap());
        assert_eq!(
            extract_client_ip(&req, true, &proxies, true),
            Some(IpAddr::from_str("192.168.1.1").unwrap())
        );

        // Trust forwarded headers, peer is trusted
        let mut req = Request::new(());
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::from_str("10.0.0.1").unwrap(),
            80,
        )));
        req.headers_mut()
            .insert("x-forwarded-for", "8.8.8.8".parse().unwrap());
        assert_eq!(
            extract_client_ip(&req, true, &proxies, true),
            Some(IpAddr::from_str("8.8.8.8").unwrap())
        );

        // Trust forwarded headers, peer is trusted, X-Real-IP used
        let mut req = Request::new(());
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::from_str("10.0.0.1").unwrap(),
            80,
        )));
        req.headers_mut()
            .insert("x-real-ip", "8.8.8.8".parse().unwrap());
        assert_eq!(
            extract_client_ip(&req, true, &proxies, true),
            Some(IpAddr::from_str("8.8.8.8").unwrap())
        );

        // Multiple X-Forwarded-For headers
        let mut req = Request::new(());
        req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::from_str("10.0.0.1").unwrap(),
            80,
        )));
        req.headers_mut()
            .append("x-forwarded-for", "8.8.8.8".parse().unwrap());
        req.headers_mut()
            .append("x-forwarded-for", "10.0.0.2".parse().unwrap());
        assert_eq!(
            extract_client_ip(&req, true, &proxies, true),
            Some(IpAddr::from_str("10.0.0.2").unwrap()) // 10.0.0.2 is the first untrusted from the right
        );

        // SocketAddr fallback
        let mut req = Request::new(());
        req.extensions_mut().insert(SocketAddr::new(
            IpAddr::from_str("192.168.1.1").unwrap(),
            80,
        ));
        assert_eq!(
            extract_client_ip(&req, false, &proxies, false),
            Some(IpAddr::from_str("192.168.1.1").unwrap())
        );
    }
}
