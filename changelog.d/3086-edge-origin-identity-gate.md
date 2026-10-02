### Security

- `#[edge(needs(identity))]` routes are now identity-gated on the native
  origin mount too (issue #3086). Previously the capsule declined
  unauthenticated requests before dispatch, but the origin fallback served
  them anyway, so a declaration-only identity route was reachable without
  credentials. The origin mount now runs `autumn_edge::require_edge_identity`,
  returning the same 401 + `x-autumn-edge-fallthrough: missing_capability`
  rejection the `EdgeIdentity` extractor returns.
