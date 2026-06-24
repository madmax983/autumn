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
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_trusted_proxy_parse() {
        let mut tp = TrustedProxy::parse("10.0.0.0/8").unwrap();
        assert_eq!(tp.network, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)));
        assert_eq!(tp.prefix_len, 8);

        tp = TrustedProxy::parse("192.168.1.1").unwrap();
        assert_eq!(tp.network, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(tp.prefix_len, 32);

        tp = TrustedProxy::parse("2001:db8::/32").unwrap();
        assert_eq!(
            tp.network,
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0))
        );
        assert_eq!(tp.prefix_len, 32);

        tp = TrustedProxy::parse("2001:db8::1").unwrap();
        assert_eq!(
            tp.network,
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))
        );
        assert_eq!(tp.prefix_len, 128);

        assert!(TrustedProxy::parse("").is_none());
        assert!(TrustedProxy::parse("  ").is_none());
        assert!(TrustedProxy::parse("10.0.0.0/33").is_none());
        assert!(TrustedProxy::parse("::1/129").is_none());
        assert!(TrustedProxy::parse("invalid").is_none());
        assert!(TrustedProxy::parse("10.0.0.0/invalid").is_none());
    }

    #[test]
    fn test_trusted_proxy_contains() {
        let tp_v4 = TrustedProxy::parse("10.0.0.0/8").unwrap();
        assert!(tp_v4.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!tp_v4.contains(IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1))));
        assert!(!tp_v4.contains(IpAddr::V6(Ipv6Addr::LOCALHOST)));

        let tp_v6 = TrustedProxy::parse("2001:db8::/32").unwrap();
        assert!(tp_v6.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))));
        assert!(!tp_v6.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db9, 0, 0, 0, 0, 0, 1))));
        assert!(!tp_v6.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));

        let tp_v4_any = TrustedProxy::parse("0.0.0.0/0").unwrap();
        assert!(tp_v4_any.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(tp_v4_any.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(!tp_v4_any.contains(IpAddr::V6(Ipv6Addr::LOCALHOST)));

        let tp_v6_any = TrustedProxy::parse("::/0").unwrap();
        assert!(tp_v6_any.contains(IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))));
        assert!(!tp_v6_any.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_extract_client_ip() {
        use axum::http::Request;

        // Trust forwarded headers == false
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, false, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );

        // Trust forwarded headers == true, no proxies configured
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // Trust forwarded headers == true, proxy configured and trusted
        let tp = TrustedProxy::parse("10.0.0.0/8").unwrap();
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[tp], true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // Trust forwarded headers == true, proxy configured and NOT trusted
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[tp], true),
            Some(IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1)))
        );

        // Multiple XFF headers
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .header("x-forwarded-for", "2.2.2.2")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2)))
        );

        // X-Real-IP fallback
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-real-ip", "1.1.1.1")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // Neither XFF nor X-Real-IP
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );

        // Invalid X-Real-IP
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-real-ip", "invalid")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );

        // XFF takes precedence over X-Real-IP
        let req = Request::builder()
            .extension(ConnectInfo(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                8080,
            )))
            .header("x-forwarded-for", "1.1.1.1")
            .header("x-real-ip", "2.2.2.2")
            .body(())
            .unwrap();
        assert_eq!(
            extract_client_ip(&req, true, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );
    }

    #[test]
    fn test_client_ip_from_x_forwarded_for() {
        // Not configured: returns rightmost or prev if rightmost == peer
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1", None, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1, 2.2.2.2", None, &[], false),
            Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2)))
        );
        assert_eq!(
            client_ip_from_x_forwarded_for(
                "1.1.1.1, 2.2.2.2",
                Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2))),
                &[],
                false
            ),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );
        assert_eq!(
            client_ip_from_x_forwarded_for("invalid", None, &[], false),
            None
        );
        assert_eq!(
            client_ip_from_x_forwarded_for(
                "1.1.1.1, invalid",
                Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2))),
                &[],
                false
            ),
            None
        );
        assert_eq!(
            client_ip_from_x_forwarded_for(
                "invalid, 2.2.2.2",
                Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2))),
                &[],
                false
            ),
            Some(IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2))) // `prev.parse` fails, returns last_ip
        );
        assert_eq!(client_ip_from_x_forwarded_for("", None, &[], false), None);

        // Configured: walks right-to-left until untrusted proxy found
        let tp1 = TrustedProxy::parse("10.0.0.0/8").unwrap();
        let tp2 = TrustedProxy::parse("192.168.0.0/16").unwrap();
        let proxies = vec![tp1, tp2];

        // 1.1.1.1 is untrusted
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1", None, &proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // 10.0.0.1 is trusted, so it skips to next which is 1.1.1.1
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1, 10.0.0.1", None, &proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // 10.0.0.1 and 192.168.1.1 are trusted, skips to 1.1.1.1
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1, 192.168.1.1, 10.0.0.1", None, &proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );

        // All are trusted, none untrusted found, returns None
        assert_eq!(
            client_ip_from_x_forwarded_for("192.168.1.1, 10.0.0.1", None, &proxies, true),
            None
        );

        // Invalid IP encountered during traversal - it skips it and returns None (as there is no other ip to check after)
        assert_eq!(
            client_ip_from_x_forwarded_for("1.1.1.1, invalid, 10.0.0.1", None, &proxies, true),
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );
    }

    #[test]
    fn test_is_trusted_proxy() {
        let tp1 = TrustedProxy::parse("10.0.0.0/8").unwrap();
        let tp2 = TrustedProxy::parse("192.168.0.0/16").unwrap();
        let proxies = vec![tp1, tp2];

        assert!(is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            &proxies
        ));
        assert!(is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &proxies
        ));
        assert!(!is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(11, 0, 0, 1)),
            &proxies
        ));
    }

    #[test]
    fn test_mutants_coverage() {
        use axum::http::Request;
        let req = Request::builder()
            .header("x-forwarded-for", "")
            .body(())
            .unwrap();
        assert_eq!(extract_client_ip(&req, true, &[], true), None);

        let req = Request::builder().header("x-real-ip", "").body(()).unwrap();
        assert_eq!(extract_client_ip(&req, true, &[], true), None);

        assert_eq!(
            client_ip_from_x_forwarded_for(
                "invalid, 10.0.0.1",
                Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
                &[],
                false
            ),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );
    }
}
