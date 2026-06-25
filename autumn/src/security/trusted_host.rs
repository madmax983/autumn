use crate::config::AutumnConfig;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct TrustedHostPolicy {
    rules: Arc<Vec<String>>,
    allow_any: bool,
    pub(crate) allow_missing_host: bool,
    pub(crate) probe_bypass_paths: Arc<std::collections::HashSet<String>>,
}

impl TrustedHostPolicy {
    pub fn from_config(config: &AutumnConfig) -> Self {
        let mut rules: Vec<String> = config
            .security
            .trusted_hosts
            .hosts
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .map(|h| h.trim_end_matches('.').to_owned())
            .filter(|h| !h.is_empty())
            .collect();
        let is_production = matches!(config.profile.as_deref(), Some("prod" | "production"));
        if !is_production {
            rules.extend(
                ["localhost", "127.0.0.1", "::1"]
                    .into_iter()
                    .map(std::borrow::ToOwned::to_owned),
            );
        }
        let allow_any = rules.iter().any(|h| h == "*");
        let probe_bypass_paths = std::collections::HashSet::from([
            config.health.path.clone(),
            config.health.live_path.clone(),
            config.health.ready_path.clone(),
            config.health.startup_path.clone(),
            crate::actuator::actuator_route_path(&config.actuator.prefix, "/health"),
        ]);
        Self {
            rules: Arc::new(rules),
            allow_any,
            allow_missing_host: !is_production,
            probe_bypass_paths: Arc::new(probe_bypass_paths),
        }
    }

    /// Whether a request carrying no usable `Host` is allowed through. Mirrors
    /// `trusted_host_middleware`'s missing-host branch for callers (e.g. the MCP
    /// envelope) that enforce the policy outside that middleware.
    ///
    /// Only the `mcp` feature consumes this today; gated so default-feature
    /// builds don't flag it as dead code.
    #[cfg(feature = "mcp")]
    #[must_use]
    pub const fn allows_missing_host(&self) -> bool {
        self.allow_missing_host
    }

    #[must_use]
    pub fn allows_host(&self, host: &str) -> bool {
        if self.allow_any {
            return true;
        }
        self.rules.iter().any(|rule| {
            rule.strip_prefix('.').map_or_else(
                || host == rule,
                |suffix| {
                    host == suffix
                        || host
                            .strip_suffix(suffix)
                            .is_some_and(|prefix| prefix.ends_with('.'))
                },
            )
        })
    }
}

#[must_use]
pub fn extract_host_without_port(header: &str) -> Option<&str> {
    let host = header.trim();
    if host.is_empty() {
        return None;
    }
    if host.starts_with('[') {
        let end = host.find(']')?;
        let literal = host.get(1..end)?;
        if literal.is_empty() || literal.parse::<std::net::IpAddr>().is_err() {
            return None;
        }

        let remainder = host.get(end + 1..)?;
        if remainder.is_empty() {
            return Some(literal);
        }

        let maybe_port = remainder.strip_prefix(':')?;
        if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) {
            return Some(literal);
        }

        return None;
    }
    let Some((candidate, maybe_port)) = host.rsplit_once(':') else {
        return Some(host);
    };
    if candidate.contains(':') {
        // unbracketed IPv6 literal; keep host verbatim
        return Some(host);
    }
    if !maybe_port.is_empty()
        && maybe_port.chars().all(|c| c.is_ascii_digit())
        && !candidate.is_empty()
    {
        Some(candidate)
    } else {
        None
    }
}
