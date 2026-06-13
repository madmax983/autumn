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

## In-memory log capture (`/actuator/logfile`)

Autumn can buffer recent structured log entries in memory and expose them via the
`/actuator/logfile` endpoint — useful for inspecting application log output without
SSH access or an external aggregator.

### Enabling

```toml
[log.capture]
enabled  = true   # default: false
capacity = 1000   # max entries retained (ring buffer; default: 1000)
```

The endpoint requires the sensitive actuator to be enabled:

```toml
[actuator]
sensitive = true   # required; always on in the "dev" profile
```

### Querying

```
GET /actuator/logfile
GET /actuator/logfile?level=warn
GET /actuator/logfile?level=error&limit=50
```

| Parameter | Description |
|-----------|-------------|
| `level`   | Minimum severity to return: `trace`, `debug`, `info`, `warn`, or `error` (case-insensitive). Returns `400 Bad Request` for unrecognised values. Omit to return all levels. |
| `limit`   | Cap the response to the most-recent *N* entries. Omit to return all retained entries. |

Results are returned in chronological order (oldest first), newest-last.

### Response shape

```json
{
  "capture_enabled": true,
  "total": 312,
  "entries": [
    {
      "timestamp": "2026-01-15T12:34:56.789Z",
      "level": "INFO",
      "target": "myapp::orders",
      "message": "order placed",
      "fields": { "order_id": "A-1001", "user_id": "42" },
      "request_id": "req-abc123"
    }
  ]
}
```

When `log.capture.enabled = false` (the default), the endpoint still responds with
`200` and `"capture_enabled": false` so API consumers can handle the case uniformly.

### Request context fields

When a log event is emitted inside a request, the capture layer automatically
includes `request_id`, `user_id`, `tenant_id`, and any custom fields set via
`LogContext` (e.g. `ctx.set_user_id("42")` or `ctx.insert_field("region", "eu-1")`).
Fields on the tracing event take priority over the same key from the request context.

### Security

The capture buffer uses the same scrubber as the rest of the logging pipeline:

- Sensitive field values (passwords, tokens, SSNs, …) are replaced with
  `[FILTERED]` **before** storage — they never enter the buffer.
- If your app uses `#[model]` encrypted columns, their names are automatically
  added to the scrubber so plaintext values are filtered even if not listed in
  `log.filter_parameters`.
- The endpoint is only reachable when `actuator.sensitive = true` (off by default
  in production profiles).

