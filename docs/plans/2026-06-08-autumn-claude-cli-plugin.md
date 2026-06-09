# Autumn Claude CLI Plugin Design

## Goal

Package Autumn guidance as a real Claude-facing plugin that delegates framework
operations to the `autumn` CLI instead of duplicating generator, doctor, route,
migration, and runtime-control logic in prompt text.

The plugin should make Claude better at Autumn work by calling stable CLI
commands, reading machine-readable output where available, and falling back to
source inspection only when the CLI lacks the needed contract.

## Plugin Shape

The plugin should contain:

- A compact Autumn skill that explains when to use the plugin and how to choose
  CLI commands.
- CLI wrapper scripts for command discovery, JSON normalization, and safe
  execution.
- Optional MCP/server glue only if Claude's current plugin host supports local
  command tools with explicit allowlists.
- Reference files generated from `autumn --help` and selected subcommand help
  text, refreshed during release prep.

Do not reimplement CLI behavior in the plugin. If Claude needs to scaffold,
inspect routes, check migrations, or toggle feature flags, it should run the
Autumn CLI and summarize results.

## Command Contract

Safe read-only commands:

- `autumn --help`
- `autumn doctor --json`
- `autumn routes --format json --user-only`
- `autumn migrate check`
- `autumn config list`
- `autumn flags list`
- `autumn experiments list`
- `autumn dev-loop-bench --dry-run --json`

Project-mutating commands requiring explicit confirmation:

- `autumn new`
- `autumn generate model`
- `autumn generate scaffold`
- `autumn generate auth`
- `autumn generate mailer`
- `autumn generate system-test`
- `autumn release init`
- `autumn credentials edit`
- `autumn maintenance on|off`
- `autumn flags enable|disable|set-rollout|allow`
- `autumn experiments set-weights|conclude|override`

Commands that touch live HTTP/database state must include the resolved project
root, profile, database/source URL if relevant, and a redacted command preview
before execution.

## CLI Output Rules

- Prefer JSON flags whenever the CLI offers them.
- Capture stdout, stderr, exit code, cwd, and elapsed time.
- Redact secrets from env vars, URLs, headers, credentials output, and command
  arguments before showing Claude or the user.
- Treat nonzero exits as evidence, not as prompt-level failure. Claude should
  summarize the failed check and point to the adjacent fix.

## Initial Plugin Tools

1. `autumn_doctor`
   Runs `autumn doctor --json` and returns structured findings.

2. `autumn_routes`
   Runs `autumn routes --format json`, with optional `--user-only`,
   `--filter`, and `--method`.

3. `autumn_migration_check`
   Runs `autumn migrate check` to classify migration risk before deploy work.

4. `autumn_generate`
   Builds a redacted command preview for generator commands and executes only
   after explicit approval.

5. `autumn_runtime`
   Wraps runtime config, feature flag, experiment, maintenance, token, and data
   commands with stricter confirmation and redaction.

## Skill Updates

The plugin skill should teach these rules:

- Use `autumn` CLI commands before reconstructing project state manually.
- Use `doctor`, `routes`, and `migrate check` as the first triage layer for
  deployment or generated-app issues.
- Use `generate auth --oauth` as a supported 0.5.0 path; OAuth2/OIDC was
  reapplied after its temporary revert.
- Do not answer release-summary questions from `CHANGELOG.md` alone when the
  branch has `Unreleased` content above a release heading; compare the PR head
  and release notes.

## Validation

Minimum test matrix:

- Run the plugin against `examples/hello` and verify `doctor --json` and
  `routes --format json` return parseable output.
- Run generator dry-run or fixture-backed commands for auth, scaffold, mailer,
  and system-test workflows.
- Verify command previews redact secrets and mark mutating commands as requiring
  approval.
- Verify the plugin refuses or pauses before live state changes such as
  `maintenance on`, credentials editing, token revocation, or feature flag
  mutation.

## Open Decision

Pick the host packaging format after checking Claude's current plugin contract.
The Autumn-specific contract above should survive either a Claude Code plugin,
a Codex plugin, or a local MCP bridge because the CLI remains the stable
boundary.
