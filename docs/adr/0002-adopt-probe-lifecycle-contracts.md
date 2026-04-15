# ADR 0002: Adopt Explicit Probe Lifecycle Contracts

- Status: Proposed
- Date: 2026-04-09
- Deciders: Autumn maintainers
- Tags: health, probes, operations, cloud-native

## Context

Autumn currently exposes `/health` and `/actuator/health`, but the effective
contract is still a single health endpoint. That was acceptable for early
framework development, but it is too vague for orchestrated environments.

Load balancers, Kubernetes, and other schedulers need distinct answers to
distinct questions:

- Is the process alive?
- Is it ready to receive traffic?
- Has startup actually completed?

One endpoint cannot answer all three cleanly without becoming a semantic mud
puddle.

Autumn also needs better shutdown semantics. During graceful shutdown, Autumn
should stop receiving new traffic before it stops being alive. That means
readiness should fail before liveness.

## Decision

Autumn will adopt explicit lifecycle probes:

- `/live`
- `/ready`
- `/startup`

Autumn will retain `/health` during the transition as a compatibility endpoint
for `v0.x`.

## Probe Semantics

### `/live`

`/live` answers: "Should the runtime be restarted?"

It should return `200 OK` when:

- the process is running
- the router is mounted
- the runtime has not entered a terminally broken state

It should not fail merely because an external dependency is unavailable. A dead
database is a readiness problem first, not a liveness problem.

### `/ready`

`/ready` answers: "Is this replica safe to receive new traffic?"

It should return `200 OK` only when all required serving dependencies are ready.
That includes, when configured:

- database connectivity or pool availability
- startup hook completion
- migration gate completion if the app is configured to wait for migrations
- required external state backends such as Redis-backed sessions
- registered custom readiness checks

During graceful shutdown, `/ready` must flip to `503` before the server begins
draining.

### `/startup`

`/startup` answers: "Has bootstrapping completed yet?"

It should return `503` until startup hooks and required bootstrap gates have
completed. Once startup succeeds, it should return `200` and remain successful
unless the process is restarted.

## Extensibility

Autumn will provide an application-level readiness registration seam for custom
checks.

It will not provide arbitrary custom liveness hooks in Phase 1. Liveness should
remain narrow and hard to abuse.

## Configuration

Autumn will add explicit probe configuration with defaults:

- `live_path = "/live"`
- `ready_path = "/ready"`
- `startup_path = "/startup"`
- compatibility `health_path = "/health"`

The runtime must honor configured paths consistently across routing, startup
logs, and docs.

## Consequences

### Positive

- Operators get probe semantics that match how orchestrators actually work
- Graceful shutdown becomes easier to reason about
- Readiness can model real production dependencies
- Autumn moves away from ambiguous health semantics

### Negative

- More endpoints must be documented and maintained
- Probe state becomes a first-class part of `AppState`
- Users who only know `/health` need migration guidance

### Risks

- Allowing readiness to become too slow or dependency-heavy
- Confusing users if `/health` and `/ready` diverge without clear transition docs
- Overloading liveness with dependency checks and causing restart storms

## Alternatives Considered

### 1. Keep only `/health`

Rejected because it cannot express startup, readiness, and liveness cleanly.

### 2. Replace `/health` immediately

Rejected for now because Autumn still has existing users and examples relying on
`/health`. A compatibility window is cheaper than a rude surprise.

### 3. Make all probe semantics user-defined

Rejected because operators need consistent defaults across Autumn applications.
If every app reinvents probe meaning, the framework contributes nothing.

## Non-Goals

- Building a full health-check framework for arbitrary business diagnostics
- Turning liveness into a dependency monitor
- Adding per-endpoint authentication for probes in Phase 1

## Follow-On Work

- Implement probe lifecycle state in the runtime
- Add readiness check registration to `AppBuilder`
- Update CLI templates and docs to show probe usage
