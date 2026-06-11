# Logging & PII

Autumn includes a parameter scrubber for structured payloads. Today, it is wired
into dev HTML error-badge request context rendering (headers/query) and helper APIs.
It is **not yet globally applied to every tracing/log event payload**.

## Built-in defaults

By default, the scrubber filters keys such as:

- `password`, `password_confirmation`
- `token`, `access_token`, `refresh_token`
- `secret`, `authorization`
- `api_key`
- `cookie`, `set-cookie`
- `ssn`, `credit_card`, `card_number`, `cvv`

Matched values are replaced with:

```text
[FILTERED]
```

## Configure in `autumn.toml`

```toml
[log]
level = "info"
format = "Json"

# Add app-specific sensitive keys
filter_parameters = ["pin", "private_note"]

# Opt out of built-in defaults (use sparingly)
unfilter_parameters = ["password"]
```

### Important behavior

- Matching is case-insensitive.
- Matching is normalization-aware for separators/casing (`api_key`, `apiKey`,
  `API-KEY`, `apikey` are treated equivalently).
- Empty custom keys are ignored to avoid accidental “scrub everything”.

## Access log

Every served HTTP request emits one structured access-log line by default
(`tracing` target `autumn::access`, level `INFO`) carrying `method`, `route`
(the matched low-cardinality template, e.g. `/users/{id}` — never the raw
path), `status`, `duration_ms`, and `request_id` (the same id as the
`x-request-id` header and error pages). It renders through the standard
subscriber, so `log.format` controls its shape, and it requires no telemetry
feature or collector.

The line never includes query strings, headers, or bodies, so it cannot leak
the sensitive values this scrubber protects.

Probe and asset noise is excluded by default; both knobs live in `[log]`:

```toml
[log]
# On by default; set to false to silence the access log without recompiling.
access_log = true

# Path prefixes to skip (whole-segment match; replaces the default set:
# "/health", "/live", "/ready", "/startup", "/actuator", "/static").
access_log_exclude = ["/health", "/actuator", "/static", "/uptime-probe"]
```

Both knobs also honor environment overrides for TOML-less deployments:
`AUTUMN_LOG__ACCESS_LOG=false` and
`AUTUMN_LOG__ACCESS_LOG_EXCLUDE=/health,/internal` (comma-separated).

## Startup warnings

If you opt out of built-in sensitive defaults via `unfilter_parameters`, Autumn
emits a startup warning listing the opted-out keys.

## Programmatic use

```rust
use autumn_web::log::filter::scrub;
use serde_json::json;

let payload = json!({
    "email": "user@example.com",
    "password": "secret"
});

let scrubbed = scrub(&payload);
assert_eq!(scrubbed["password"], "[FILTERED]");
```
