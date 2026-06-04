//! Centralized trusted-proxy policy for forwarded-header middleware.
//!
//! This module provides [`ProxyResolver`] — the single source of truth for
//! evaluating `X-Forwarded-*` headers. Every framework middleware that needs
//! a "real" client IP, host, or scheme must go through this resolver instead
//! of reading forwarding headers directly.
//!
//! # Design
//!
//! Operators declare their proxy trust boundary once in `[security.trusted_proxies]`
//! and every middleware honours it automatically.  The resolver supports two
//! trust strategies:
//!
//! - **CIDR ranges** (`ranges`): walk from the right of the `X-Forwarded-For`
//!   chain, skipping IPs that fall inside a trusted range. The first untrusted
//!   IP is the real client.
//! - **Hop count** (`trusted_hops`): strip exactly N entries from the right of
//!   the chain. Useful when the exact proxy IPs are dynamic (e.g., ALB).
//!
//! When `trust_forwarded_headers = false` (the default in `prod` without config)
//! the resolver ignores all `X-Forwarded-*` regardless of the chain.
//!
//! # Profile-aware defaults
//!
//! | Profile | Default |
//! |---------|---------|
//! | `dev`   | Trust loopback only (`127.0.0.1/8`, `::1/128`) |
//! | `prod`  | No forwarding trust until configured |
//!
//! # Plugin authors
//!
//! > **Never read `X-Forwarded-*` directly. Use the `ClientAddr`, `ClientHost`,
//! > or `ClientScheme` extractors from `autumn_web::extract`.**
//!
//! These extractors are resolved by a framework-managed [`ProxyResolver`] and
//! are the only blessed way to obtain real client identity from request
//! handlers and middleware.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::Request;

use crate::security::config::TrustedProxiesConfig;

/// A parsed trusted-proxy CIDR range or exact IP.
#[derive(Debug, Clone, Copy)]
struct TrustedProxy {
    network: IpAddr,
    prefix_len: u8,
}

impl TrustedProxy {
    fn parse(value: &str) -> Option<Self> {
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

    fn contains(&self, ip: IpAddr) -> bool {
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

/// Resolves real client identity from `X-Forwarded-*` headers, honoring the
/// operator-configured trusted-proxy policy.
///
/// Construct via [`ProxyResolver::from_config`] and call the `resolve_*`
/// methods inside middleware. Framework-wired middleware receive a shared
/// resolver; plugin authors should use the [`ClientAddr`], [`ClientHost`], and
/// [`ClientScheme`] extractors from `autumn_web::extract` instead of calling
/// this resolver directly.
///
/// [`ClientAddr`]: crate::extract::ClientAddr
/// [`ClientHost`]: crate::extract::ClientHost
/// [`ClientScheme`]: crate::extract::ClientScheme
#[derive(Debug, Clone)]
pub struct ProxyResolver {
    ranges: Vec<TrustedProxy>,
    /// True when the original config had at least one range entry (even if all
    /// failed to parse).  When `true` and `ranges` is empty (all entries were
    /// invalid), we treat NO peer as trusted rather than all peers as trusted.
    ranges_configured: bool,
    trusted_hops: Option<u32>,
    trust_forwarded_headers: bool,
}

impl ProxyResolver {
    /// Construct a resolver from the operator's `[security.trusted_proxies]` config block.
    #[must_use]
    pub fn from_config(config: &TrustedProxiesConfig) -> Self {
        let ranges_configured = !config.ranges.is_empty();
        let ranges = config
            .ranges
            .iter()
            .filter_map(|proxy| {
                TrustedProxy::parse(proxy).or_else(|| {
                    tracing::warn!(
                        range = %proxy,
                        "ignoring invalid trusted_proxies range"
                    );
                    None
                })
            })
            .collect();

        Self {
            ranges,
            ranges_configured,
            trusted_hops: config.trusted_hops,
            trust_forwarded_headers: config.trust_forwarded_headers,
        }
    }

    /// Returns `true` when `ip` is a trusted proxy per the configured ranges.
    ///
    /// When no ranges are configured (`ranges_configured = false`), returns
    /// `true` for any IP (trust all peers). When ranges were configured but all
    /// failed to parse, returns `false` for any IP (trust no peers).
    fn is_trusted_ip(&self, ip: IpAddr) -> bool {
        if !self.ranges_configured {
            return true;
        }
        self.ranges.iter().any(|r| r.contains(ip))
    }

    /// Build a resolver that trusts loopback addresses only (dev-profile default).
    #[must_use]
    pub fn loopback_only() -> Self {
        Self::from_config(&TrustedProxiesConfig {
            ranges: vec!["127.0.0.0/8".to_owned(), "::1/128".to_owned()],
            trusted_hops: None,
            trust_forwarded_headers: true,
        })
    }

    /// Build a resolver that trusts no forwarding headers (prod default when unconfigured).
    #[must_use]
    pub fn no_trust() -> Self {
        Self {
            ranges: Vec::new(),
            ranges_configured: false,
            trusted_hops: None,
            trust_forwarded_headers: false,
        }
    }

    /// Resolve the real client IP address from the request.
    ///
    /// When `trust_forwarded_headers` is `false`, returns the TCP peer IP.
    /// When `trust_forwarded_headers` is `true`:
    ///   - If `trusted_hops` is set, peels exactly that many entries from the
    ///     right of the `X-Forwarded-For` chain.
    ///   - If CIDR ranges are configured, walks from the right skipping trusted
    ///     IPs and returns the first untrusted IP.
    ///   - If neither is configured, falls back to the rightmost entry
    ///     (single-proxy assumption), then `X-Real-IP`, then peer IP.
    pub fn resolve_client_addr<B>(&self, req: &Request<B>) -> Option<IpAddr> {
        let peer_ip = Self::peer_ip(req);

        if !self.trust_forwarded_headers {
            return peer_ip;
        }

        // Honour forwarding headers only when the immediate peer is trusted.
        // "No ranges configured" means trust all peers.
        // "Ranges configured but all invalid" means trust no peers.
        let peer_is_trusted = peer_ip.is_some_and(|ip| self.is_trusted_ip(ip))
            || (!self.ranges_configured && peer_ip.is_none());

        if !peer_is_trusted {
            return peer_ip;
        }

        if let Some(xff) = Self::x_forwarded_for(req) {
            if let Some(hops) = self.trusted_hops {
                // Peel exactly `hops` entries from the right; the next one is the client.
                let entries: Vec<&str> = xff
                    .rsplit(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();

                let client_idx = hops as usize;
                if let Some(ip) = entries.get(client_idx).and_then(|s| s.parse::<IpAddr>().ok()) {
                    return Some(ip);
                }
                return peer_ip;
            }

            if self.ranges_configured {
                for entry in xff.rsplit(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let Ok(ip) = entry.parse::<IpAddr>() else {
                        continue;
                    };
                    if !self.ranges.iter().any(|r| r.contains(ip)) {
                        return Some(ip);
                    }
                }
                return peer_ip;
            }

            // No ranges, no hop count: use rightmost entry, but skip if it equals
            // the peer (i.e. the proxy appended itself).
            let mut entries = xff.rsplit(',').map(str::trim).filter(|s| !s.is_empty());
            if let Some(rightmost) = entries.next() {
                if !rightmost.is_empty() {
                    if peer_ip.is_some_and(|p| rightmost.parse::<IpAddr>().is_ok_and(|ip| ip == p)) {
                        if let Some(prev) = entries.next() {
                            return prev.parse().ok().or_else(|| rightmost.parse().ok());
                        }
                        return rightmost.parse().ok();
                    }
                    if let Ok(ip) = rightmost.parse::<IpAddr>() {
                        return Some(ip);
                    }
                }
            }
        }

        // XFF absent or empty — fall through to X-Real-IP, then peer IP.
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

        peer_ip
    }

    /// Resolve the external host as seen by the client.
    ///
    /// Returns `X-Forwarded-Host` when `trust_forwarded_headers` is `true` and
    /// the peer is trusted; falls back to the `Host` header.
    pub fn resolve_client_host<B>(&self, req: &Request<B>) -> Option<String> {
        let peer_ip = Self::peer_ip(req);
        let peer_is_trusted =
            peer_ip.is_some_and(|ip| self.is_trusted_ip(ip)) || !self.ranges_configured;

        if self.trust_forwarded_headers && peer_is_trusted {
            if let Some(fwd_host) = req
                .headers()
                .get("x-forwarded-host")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(fwd_host.to_owned());
            }
        }

        req.headers()
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_owned())
    }

    /// Resolve the external scheme (`"http"` or `"https"`) as seen by the client.
    ///
    /// Returns the leftmost value of `X-Forwarded-Proto` when
    /// `trust_forwarded_headers` is `true` and the peer is trusted;
    /// otherwise falls back to the request URI scheme, then `"http"`.
    pub fn resolve_client_scheme<B>(&self, req: &Request<B>) -> String {
        let peer_ip = Self::peer_ip(req);
        let peer_is_trusted =
            peer_ip.is_some_and(|ip| self.is_trusted_ip(ip)) || !self.ranges_configured;

        if self.trust_forwarded_headers && peer_is_trusted {
            if let Some(proto) = req
                .headers()
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
            {
                // Multiple values are comma-separated; the leftmost is the
                // client-facing scheme.
                let outermost = proto.split(',').next().unwrap_or(proto).trim();
                if !outermost.is_empty() {
                    return outermost.to_ascii_lowercase();
                }
            }
        }

        req.uri()
            .scheme_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "http".to_owned())
    }

    fn peer_ip<B>(req: &Request<B>) -> Option<IpAddr> {
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip())
    }

    fn x_forwarded_for<B>(req: &Request<B>) -> Option<String> {
        let all: Vec<&str> = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();

        if all.is_empty() {
            None
        } else {
            Some(all.join(", "))
        }
    }
}

/// Resolved client identity, injected into request extensions by the
/// framework's proxy-resolver middleware.  Extractors read from this.
#[derive(Debug, Clone)]
pub struct ResolvedClientIdentity {
    /// Resolved client IP (after trust evaluation).
    pub addr: Option<IpAddr>,
    /// Resolved external host.
    pub host: Option<String>,
    /// Resolved external scheme (`"http"` or `"https"`).
    pub scheme: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn req_with_xff(xff: &str) -> Request<()> {
        Request::builder()
            .header("x-forwarded-for", xff)
            .body(())
            .unwrap()
    }

    fn req_with_peer_and_xff(peer: &str, xff: &str) -> Request<()> {
        let addr: SocketAddr = format!("{peer}:1234").parse().unwrap();
        let mut req = Request::builder()
            .header("x-forwarded-for", xff)
            .body(())
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    fn req_with_peer(peer: &str) -> Request<()> {
        let addr: SocketAddr = format!("{peer}:1234").parse().unwrap();
        let mut req = Request::builder().body(()).unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    // ── TrustedProxy parsing ───────────────────────────────────────────────

    #[test]
    fn trusted_proxy_parse_exact_ipv4() {
        let p = TrustedProxy::parse("10.0.0.1").unwrap();
        assert!(p.contains("10.0.0.1".parse().unwrap()));
        assert!(!p.contains("10.0.0.2".parse().unwrap()));
    }

    #[test]
    fn trusted_proxy_parse_cidr() {
        let p = TrustedProxy::parse("10.0.0.0/24").unwrap();
        assert!(p.contains("10.0.0.1".parse().unwrap()));
        assert!(p.contains("10.0.0.254".parse().unwrap()));
        assert!(!p.contains("10.0.1.0".parse().unwrap()));
    }

    #[test]
    fn trusted_proxy_parse_invalid_returns_none() {
        assert!(TrustedProxy::parse("not-an-ip").is_none());
        assert!(TrustedProxy::parse("").is_none());
        assert!(TrustedProxy::parse("10.0.0.0/33").is_none());
    }

    // ── AC (a): attacker-controlled leading value rejected with trusted_hops=1 ──

    #[test]
    fn trusted_hops_one_rejects_attacker_controlled_leading_value() {
        // X-Forwarded-For: <attacker-injected>, <real-client>, <alb>
        // With trusted_hops=1 we peel 1 entry from the right (the ALB).
        // The real client is at position 1, NOT the attacker-injected value at 0.
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: Some(1),
            trust_forwarded_headers: true,
        });

        let req = req_with_xff("1.2.3.4, 5.6.7.8, 10.0.0.1");
        // Peel 1 from right (10.0.0.1). Next is 5.6.7.8 (real client).
        // Attacker-injected 1.2.3.4 is NOT returned.
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "5.6.7.8".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn trusted_hops_zero_uses_rightmost_entry() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: Some(0),
            trust_forwarded_headers: true,
        });

        let req = req_with_xff("1.2.3.4, 5.6.7.8");
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "5.6.7.8".parse::<IpAddr>().unwrap());
    }

    // ── AC (b): two-hop CDN + ALB correctly identifies real client ────────────

    #[test]
    fn two_hop_cdn_alb_chain_identifies_real_client() {
        // CDN (203.0.113.10) -> ALB (10.0.1.100) -> app
        // X-Forwarded-For: <real-client>, 203.0.113.10, 10.0.1.100
        // Trusted ranges: CDN range + ALB range
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: vec!["203.0.113.0/24".to_owned(), "10.0.0.0/8".to_owned()],
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        // Peer is the ALB (the immediate upstream of the app)
        let req = req_with_peer_and_xff("10.0.1.100", "192.0.2.1, 203.0.113.10, 10.0.1.100");
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(
            addr,
            "192.0.2.1".parse::<IpAddr>().unwrap(),
            "real client must be identified correctly in a two-hop CDN+ALB chain"
        );
    }

    // ── AC (c): trust_forwarded_headers=false ignores all X-Forwarded-* ──────

    #[test]
    fn trust_forwarded_headers_false_ignores_xff() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: vec!["10.0.0.0/8".to_owned()],
            trusted_hops: None,
            trust_forwarded_headers: false,
        });

        // Peer is 10.0.0.1 (trusted range), but trust_forwarded_headers=false
        // so the XFF chain must be ignored; we return the peer IP.
        let req = req_with_peer_and_xff("10.0.0.1", "192.0.2.1, 203.0.113.10");
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(
            addr,
            "10.0.0.1".parse::<IpAddr>().unwrap(),
            "trust_forwarded_headers=false must ignore X-Forwarded-For"
        );
    }

    #[test]
    fn trust_forwarded_headers_false_ignores_x_forwarded_host() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: false,
        });

        let mut req = Request::builder()
            .header("host", "real.example")
            .header("x-forwarded-host", "attacker.example")
            .body(())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));

        let host = resolver.resolve_client_host(&req).unwrap();
        assert_eq!(host, "real.example");
    }

    #[test]
    fn trust_forwarded_headers_false_ignores_x_forwarded_proto() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: false,
        });

        let req = Request::builder()
            .header("x-forwarded-proto", "https")
            .uri("http://example.com/")
            .body(())
            .unwrap();

        let scheme = resolver.resolve_client_scheme(&req);
        assert_eq!(scheme, "http");
    }

    // ── Host and scheme resolution ──────────────────────────────────────────

    #[test]
    fn resolve_scheme_from_forwarded_proto_leftmost() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        // Multiple proxies — leftmost (client-facing) is https
        let req = Request::builder()
            .header("x-forwarded-proto", "https, http")
            .body(())
            .unwrap();
        assert_eq!(resolver.resolve_client_scheme(&req), "https");
    }

    #[test]
    fn resolve_host_prefers_forwarded_host_when_trusted() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        let req = Request::builder()
            .header("host", "internal.cluster.local")
            .header("x-forwarded-host", "public.example.com")
            .body(())
            .unwrap();
        let host = resolver.resolve_client_host(&req).unwrap();
        assert_eq!(host, "public.example.com");
    }

    #[test]
    fn resolve_host_falls_back_to_host_header() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        let req = Request::builder()
            .header("host", "app.example.com")
            .body(())
            .unwrap();
        let host = resolver.resolve_client_host(&req).unwrap();
        assert_eq!(host, "app.example.com");
    }

    // ── Untrusted peer ignores forwarding headers ───────────────────────────

    #[test]
    fn untrusted_peer_ignores_forwarding_headers() {
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: vec!["10.0.0.0/8".to_owned()],
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        // Peer is 203.0.113.1 which is NOT in trusted ranges.
        let req = req_with_peer_and_xff("203.0.113.1", "192.0.2.1");
        // Must return the peer IP, not the XFF value.
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "203.0.113.1".parse::<IpAddr>().unwrap());
    }

    // ── No peer IP (e.g. in tests without ConnectInfo) ─────────────────────

    #[test]
    fn no_peer_ip_with_trust_enabled_falls_back_to_xff() {
        // When no ConnectInfo (no peer known), and ranges is empty (trust all peers),
        // use XFF.
        let resolver = ProxyResolver::from_config(&TrustedProxiesConfig {
            ranges: Vec::new(),
            trusted_hops: None,
            trust_forwarded_headers: true,
        });

        let req = req_with_xff("192.0.2.1, 10.0.0.1");
        // No ConnectInfo; no peer to skip; no ranges configured -> trust any peer.
        // Rightmost is 10.0.0.1.
        let addr = resolver.resolve_client_addr(&req);
        assert!(addr.is_some());
    }

    // ── loopback_only() and no_trust() ─────────────────────────────────────

    #[test]
    fn loopback_only_trusts_loopback_xff() {
        let resolver = ProxyResolver::loopback_only();
        let req = req_with_peer_and_xff("127.0.0.1", "192.0.2.1");
        // Peer is loopback (trusted), so XFF is honoured.
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "192.0.2.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn loopback_only_ignores_xff_from_non_loopback_peer() {
        let resolver = ProxyResolver::loopback_only();
        let req = req_with_peer_and_xff("10.0.0.1", "192.0.2.1");
        // Peer is NOT loopback; XFF must be ignored.
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "10.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn no_trust_always_returns_peer_ip() {
        let resolver = ProxyResolver::no_trust();
        let req = req_with_peer_and_xff("10.0.0.1", "192.0.2.1");
        let addr = resolver.resolve_client_addr(&req).unwrap();
        assert_eq!(addr, "10.0.0.1".parse::<IpAddr>().unwrap());
    }
}
