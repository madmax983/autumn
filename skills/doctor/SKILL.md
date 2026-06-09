---
name: doctor
description: >
  Use when the user runs /autumn:doctor, asks to audit their Autumn app's
  configuration, check for deployment readiness, or diagnose config problems,
  missing secrets, or stale migrations.
argument-hint: "[--strict] [--json]"
allowed-tools:
  - Bash
  - Read
---

# autumn:doctor

Run `autumn doctor` to audit the project configuration and surface unsafe
defaults, missing secrets, stale migrations, and other deployment blockers.

## Execution

Run from the project root (directory containing `autumn.toml`):

```bash
autumn doctor --json
```

If `--strict` is passed as an argument, append it:

```bash
autumn doctor --strict --json
```

Capture stdout, stderr, and exit code. A nonzero exit code means problems
were found — treat it as signal, not as a hard stop.

## Output handling

The `--json` flag emits a structured result. Parse it and present a summary
grouped by severity:

- **FAIL** items: deployment blockers — list each one with the fix action from
  the `remedy` field.
- **WARN** items: non-blocking but worth flagging — list the most important ones.
- **PASS** items: do not enumerate unless asked; just report the total count.

Format:
```
Doctor results (exit <N>):
  ✗ FAIL (N): <description> — <remedy>
  ⚠ WARN (N): <description>
  ✓ PASS (N checks)
```

If the command is not found, tell the user to install:
```bash
cargo install autumn-cli --version 0.5.0
```

## Common FAIL remedies

| Check | Remedy |
|---|---|
| `signing_secret_missing` | `export AUTUMN_SECURITY__SIGNING_SECRET="$(openssl rand -hex 32)"` |
| `pending_migrations` | Run `autumn migrate` before deployment |
| `allow_unauthorized_repository_api` | Add `policy = YourPolicy` to `#[repository]` or set `security.allow_unauthorized_repository_api = true` explicitly |
| `allow_in_process_deliver_later` | Wire `Mailer::deliver_later` to a durable queue or set the config flag explicitly |
| `webhook_replay_in_memory` | Set `security.webhooks.replay.backend = "redis"` for multi-replica prod |

## Secrets redaction

Before displaying any output, redact values that look like secrets:
URLs containing passwords, signing secret values, API keys, and tokens.
Replace with `[REDACTED]`.
