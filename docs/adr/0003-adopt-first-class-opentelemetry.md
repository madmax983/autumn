# ADR 0003: Adopt First-Class OpenTelemetry

- Status: Proposed
- Date: 2026-04-09
- Deciders: Autumn maintainers
- Tags: observability, tracing, telemetry, otel

## Context

Autumn already has structured logging, request IDs, `/actuator/metrics`, and a
Prometheus-friendly endpoint. That is a useful start, but it is not enough for
distributed operations.

Operators need to trace a request across:

- ingress
- the Autumn HTTP layer
- background task dispatch
- database access
- Harvest boundaries where durable work is involved

Today, Autumn users must wire `tracing-opentelemetry`, OTLP exporters, and
propagation middleware themselves. That breaks the Autumn value proposition:
production observability should be a framework capability, not a crate-hunt.

## Decision

Autumn will make OpenTelemetry a first-class, configuration-driven framework
feature.

Phase 1 scope:

- OTLP trace export
- W3C trace-context extraction and propagation
- service/resource metadata configuration
- graceful fallback to standard `tracing` when OTLP is disabled or fails to
  initialize in non-strict mode

Existing Prometheus-compatible metrics remain supported. Autumn will not require
OTLP metrics export to land before trace support is useful.

## Telemetry Contract

### Inbound HTTP

Autumn will extract W3C trace context from incoming requests and attach request
spans automatically.

### Outbound Correlation

Autumn will make correlation information available on responses and internal
runtime boundaries so operators can connect user-visible failures to traces.

### Background Tasks

Autumn will propagate trace context into spawned tasks where a parent context is
available and semantically appropriate.

### Database

Autumn will add spans around database pool acquisition and query execution where
the cost and implementation complexity are reasonable.

### Harvest Integration

Autumn and `autumn-harvest` integration points will preserve trace context when
handing work from the web boundary into durable workflow execution.

## Configuration

Autumn will add a `[telemetry]` section, including fields such as:

- `enabled`
- `service_name`
- `service_namespace`
- `service_version`
- `environment`
- `otlp_endpoint`
- `protocol`
- `strict`

If telemetry is disabled, Autumn behaves like today.

If telemetry initialization fails:

- `strict = false`: warn and fall back to normal logging
- `strict = true`: fail fast during startup

## Consequences

### Positive

- Operators get modern tracing without custom boilerplate
- Autumn gains a coherent observability story
- Trace context can cross HTTP, tasks, and workflow boundaries
- Local development remains simple when telemetry is disabled

### Negative

- More dependencies and feature gating in the core framework
- More runtime startup modes to test
- Trace instrumentation can add latency if implemented sloppily

### Risks

- Making telemetry mandatory or too invasive for local development
- Promising distributed trace continuity without testing real boundary cases
- Tight coupling between Autumn core and one exporter/protocol implementation

## Alternatives Considered

### 1. Keep telemetry user-wired

Rejected because it keeps one of the most important production concerns outside
the framework contract.

### 2. Support only logs and request IDs

Rejected because that is insufficient once work crosses async and service
boundaries.

### 3. Make OTLP the only logging path

Rejected because Autumn must still work well for local development and simpler
deployments.

## Non-Goals

- Replacing Prometheus support immediately
- Requiring a collector in local development
- Solving every vendor-specific telemetry quirk in the core framework

## Follow-On Work

- Add telemetry config and subscriber wiring
- Add propagation tests for HTTP, task, DB, and Harvest boundaries
- Document local versus production telemetry deployment shapes
