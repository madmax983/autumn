# Verus specifications

This directory contains small mathematical shadows of critical runtime state.
They are intentionally separate from the Cargo workspace because Verus uses an
extended Rust dialect. Verify the tenant arena spine with:

```sh
verus verification/tenant_arena.rs
```

The runtime correspondence and boundary are recorded in ADR 0012; executable
tests remain authoritative for unmodeled allocator, HTTP, and concurrency glue.

## Why this is manual-only

No CI job runs Verus. Verus is not a crate you can `cargo add`: it is a
separate toolchain (the `verus` binary plus a bundled Z3 solver) with its own
release cadence and its own extended-dialect parser, and the standard CI
runners do not ship it. Installing a pinned Verus release in CI would add a
multi-hundred-megabyte download and minutes of verification time to every run
for a 109-line proof whose job is to shadow — not replace — the executable
test suite. The file is deliberately excluded from the Cargo workspace so
`cargo test --workspace` never tries to compile it. If a CI gate for it ever
becomes cheap (e.g. a cached Verus action), the job should be non-blocking
until the proof corpus is large enough to justify a red gate.
