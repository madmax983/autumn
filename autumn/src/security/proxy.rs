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

    #[test]
    fn trusted_proxy_parse_valid() {
        let tp = TrustedProxy::parse("10.0.0.1/24").unwrap();
        assert_eq!(tp.network, "10.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(tp.prefix_len, 24);

        let tp = TrustedProxy::parse("192.168.1.5").unwrap();
        assert_eq!(tp.network, "192.168.1.5".parse::<IpAddr>().unwrap());
        assert_eq!(tp.prefix_len, 32);

        let tp = TrustedProxy::parse("::1").unwrap();
        assert_eq!(tp.network, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(tp.prefix_len, 128);
    }

    #[test]
    fn trusted_proxy_parse_invalid() {
        assert!(TrustedProxy::parse("").is_none());
        assert!(TrustedProxy::parse("   ").is_none());
        assert!(TrustedProxy::parse("invalid").is_none());
        assert!(TrustedProxy::parse("10.0.0.1/invalid").is_none());
        assert!(TrustedProxy::parse("10.0.0.1/33").is_none());
        assert!(TrustedProxy::parse("::1/129").is_none());
    }

    #[test]
    fn trusted_proxy_contains() {
        let tp = TrustedProxy::parse("10.0.0.0/8").unwrap();
        assert!(tp.contains("10.1.2.3".parse().unwrap()));
        assert!(!tp.contains("11.0.0.1".parse().unwrap()));

        let tp = TrustedProxy::parse("192.168.1.5").unwrap();
        assert!(tp.contains("192.168.1.5".parse().unwrap()));
        assert!(!tp.contains("192.168.1.6".parse().unwrap()));

        let tp = TrustedProxy::parse("2001:db8::/32").unwrap();
        assert!(tp.contains("2001:db8:1234::1".parse().unwrap()));
        assert!(!tp.contains("2001:db9::".parse().unwrap()));

        // Mismatched families
        let tp = TrustedProxy::parse("10.0.0.0/8").unwrap();
        assert!(!tp.contains("::1".parse().unwrap()));

        let tp = TrustedProxy::parse("::1/128").unwrap();
        assert!(!tp.contains("127.0.0.1".parse().unwrap()));

        let tp = TrustedProxy::parse("0.0.0.0/0").unwrap();
        assert!(tp.contains("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_no_trusted_proxies_configured() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
            .extension(ConnectInfo(SocketAddr::from(([5, 6, 7, 8], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [];
        let ip = extract_client_ip(&req, true, &trusted_proxies, false);
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_no_trusted_proxies_configured_ignores_unmatched_peer() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
            .extension(ConnectInfo(SocketAddr::from(([9, 9, 9, 9], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [];
        let ip = extract_client_ip(&req, true, &trusted_proxies, false);
        assert_eq!(ip, Some("5.6.7.8".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_trusted_proxies_configured_peer_trusted() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 10.0.0.1")
            .extension(ConnectInfo(SocketAddr::from(([10, 0, 0, 2], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_trusted_proxies_configured_peer_untrusted() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 10.0.0.1")
            .extension(ConnectInfo(SocketAddr::from(([9, 9, 9, 9], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        assert_eq!(ip, Some("9.9.9.9".parse().unwrap())); // Falls back to peer IP because peer is not trusted
    }

    #[test]
    fn extract_client_ip_falls_back_to_x_real_ip() {
        let req = Request::builder()
            .header("x-real-ip", "1.2.3.4")
            .extension(ConnectInfo(SocketAddr::from(([10, 0, 0, 2], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [TrustedProxy::parse("10.0.0.0/8").unwrap()];
        let ip = extract_client_ip(&req, true, &trusted_proxies, true);
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_no_headers() {
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::from(([1, 2, 3, 4], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [];
        let ip = extract_client_ip(&req, true, &trusted_proxies, false);
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn extract_client_ip_trust_forwarded_headers_false() {
        let req = Request::builder()
            .header("x-forwarded-for", "9.9.9.9")
            .header("x-real-ip", "8.8.8.8")
            .extension(ConnectInfo(SocketAddr::from(([1, 2, 3, 4], 12345))))
            .body(())
            .unwrap();

        let trusted_proxies = [];
        let ip = extract_client_ip(&req, false, &trusted_proxies, false);
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn client_ip_from_x_forwarded_for_invalid_ip() {
        let peer_ip = Some("5.6.7.8".parse().unwrap());
        let trusted_proxies = [];
        let ip = client_ip_from_x_forwarded_for(
            "1.2.3.4, invalid, 5.6.7.8",
            peer_ip,
            &trusted_proxies,
            false,
        );
        assert_eq!(ip, Some("5.6.7.8".parse().unwrap())); // Because peer matched the last valid IP? No, the last string is 5.6.7.8 which parses.

        let ip = client_ip_from_x_forwarded_for("invalid", peer_ip, &trusted_proxies, false);
        assert_eq!(ip, None);
    }
}
