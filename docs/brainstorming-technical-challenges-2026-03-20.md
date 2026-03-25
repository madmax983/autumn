# Brainstorming Session: Technical Challenges

**Date:** 2026-03-20
**Objective:** Explore solutions for Autumn's three highest-risk technical challenges before locking PRD requirements
**Context:** Pre-PRD Phase 1 brainstorming, Level 4 project, single-developer constraint

## Techniques Used
1. Reverse Brainstorming (failure mode analysis)
2. SCAMPER (creative alternatives)
3. Six Thinking Hats (multi-perspective analysis)

---

## Challenge 1: Proc Macro Type-Checking

### Problem
Proc macros operate on token streams, not typed ASTs. When `#[get("/users")]` rewrites a function, it cannot verify extractors, return types, or trait bounds. Type errors surface in generated code with inscrutable messages.

### Failure Modes Identified
1. Turbofish nightmare — trait bound errors pointing at generated code
2. Extractor ordering — Axum's body-consuming extractor rules enforced at wrong level
3. Missing trait impls surface late (e.g., missing `Deserialize` on form struct)
4. Macro expansion debugging requires `cargo expand` (most developers don't know this)
5. Conflicting return types with unclear error paths
6. Async lifetime issues in rewritten function bodies

### Alternatives Explored
- **Wrapper function** (don't touch user code, wrap it) — preserves user-code errors
- **Trait-based adaptation** (`IntoAutumnHandler`) — compiler type-checks normally
- **Auto-apply `#[axum::debug_handler]`** in dev mode
- **Explicit return type** (`AutumnResult<T>`) instead of silent rewrite
- **Minimal macro** — only handles registration, not return type rewriting
- **Blanket trait impls** for common patterns

### Recommended Approach
**Thin wrapper + proactive `compile_error!` + explicit types in v0.1**

1. Macro generates a *wrapper function* that calls user's function unchanged
2. User's function compiles on its own merits — errors point at their code
3. Macro emits `compile_error!()` for detectable mistakes (missing async, wrong extractor position)
4. Auto-apply `#[axum::debug_handler]` in debug builds
5. Require `AutumnResult<T>` explicitly in v0.1 (consider silent rewrite in v0.2)
6. Consider layered strategy: `#[get]` (minimal) vs `#[autumn_web::handler]` (full rewrite)

### Key Principle
The macro should be a *thin wrapper*, not a deep rewrite. Ship v0.1 with minimal macro magic and add convenience in v0.2 once error messages are proven solid.

---

## Challenge 2: Linker Goblins (Route Auto-Discovery)

### Problem
`inventory` and `linkme` use linker sections for global registration. This enables magical route discovery but is platform-dependent, can silently fail, and breaks under LTO, WASM, and some static linking configurations.

### Failure Modes Identified
1. **Silent disappearance** — routes don't register, no error, just 404s
2. **LTO strips linker sections** — production binary has zero routes
3. **Order non-determinism** — conflicting routes resolve differently between builds
4. **Cross-crate discovery fails** — library crate routes not found by binary
5. **WASM incompatible** — `inventory` doesn't support WASM
6. **musl/static linking issues** — historical problems with `linkme` on musl
7. **Future Rust/linker changes** — silent breakage on toolchain updates

### Alternatives Explored
- **Build script source scanning** — `build.rs` parses `#[get]` attributes, generates `routes.rs`
- **Feature flag toggle** — linker discovery default, build-script fallback
- **Symbol-based approach** — like `wasm-bindgen`'s export mechanism
- **Named convention** — macro generates known function names, main calls them
- **`routes![]` macro** — Rocket-style explicit listing (one line per module)
- **Module-scoped scanning** — `#[autumn_web::routes]` on a module, no global discovery

### Recommended Approach
**`routes![]` macro (Rocket-style) for v0.1, linker discovery as opt-in feature flag later**

1. `#[get("/users")]` generates the handler + a registration function
2. Developer writes `routes![list_users, get_user, create_user]` once per module
3. `#[autumn_web::main]` collects from modules: `autumn_web::app().routes(users::routes()).routes(posts::routes())`
4. `routes![]` validates at compile time that listed functions have route annotations
5. Startup logs every mounted route — empty route list panics with clear error
6. Add linker-based auto-discovery behind `features = ["auto-discover"]` in v0.2+

### Key Principle
Silent failure is the kill shot. One line of explicit registration per module is not suffering; silently vanishing production routes IS. Ship boring and proven; layer magic on top once users can test across platforms.

---

## Challenge 3: The build.rs Tailwind Download Trap

### Problem
Having `build.rs` download the Tailwind standalone CLI from the internet breaks CI/CD pipelines, Nix environments, offline builds, corporate firewalls, and violates Cargo's expectation of deterministic, network-isolated build scripts.

### Failure Modes Identified
1. CI environments restrict outbound network — build fails
2. Nix/Guix sandbox network access entirely — fundamentally incompatible
3. Corporate firewalls don't whitelist Tailwind CDN
4. CDN outage / rate limiting breaks every build
5. Supply chain attack vector — executing unverified downloaded binary
6. Platform detection complexity (host vs target in cross-compilation)
7. Cargo caching — re-downloads on dependency changes without cache invalidation
8. Offline development impossible

### Alternatives Explored
- **Require PATH installation** — developer installs Tailwind themselves
- **Feature flag toggle** — opt-in download, opt-out expects PATH
- **Study `trunk`'s approach** — cached downloads with checksum verification
- **CLI-managed download** — `autumn new` downloads during scaffolding, `build.rs` uses local binary
- **Separate build command** — `cargo autumn build` or `autumn build` handles CSS
- **Skip Tailwind in v0.1** — ship with hand-written CSS defaults
- **Vendor the binary** — publish as `autumn-tailwind` crate (licensing/size concerns)
- **Watch Tailwind v4 Oxide engine** — Rust-native Tailwind compilation as future possibility

### Recommended Approach
**CLI-managed download with tiered fallback**

1. `autumn new` downloads Tailwind CLI to `target/autumn/tailwindcss` during project creation
2. `autumn setup` command for explicit tool management (download/update/verify)
3. `build.rs` checks: `target/autumn/tailwindcss` → PATH `tailwindcss` → `compile_error!("Run autumn setup or install tailwindcss")`
4. **Never** download from the internet during `cargo build`
5. Feature flag: `default-features = false` to disable Tailwind entirely
6. Monitor Tailwind v4 Oxide engine — if published as a crate, compile natively (v0.2+ opportunity)

### Key Principle
Never download from the internet during `cargo build`. Move the download to explicit CLI commands (`autumn new`, `autumn setup`). The DX promise is preserved without making every build network-dependent.

---

## Cross-Cutting Insights

### Insight 1: Magic Must Be Earned
**Description:** The v0.1 temptation is to maximize magic, but the v0.1 reality is that magic must be earned through real-world validation.
**Source:** All three techniques across all three challenges
**Impact:** High
**Effort:** Low (it's actually less work to do less magic)
**Why it matters:** Every magical feature that silently fails in production erodes trust faster than any amount of DX convenience can build it. Ship with boring, proven approaches. Layer magic on top in v0.2+ once users are testing across real environments.

### Insight 2: Failure Must Be Loud
**Description:** Silent failure is worse than boilerplate in every case. Routes that vanish, builds that break without errors, type errors in generated code — all are trust-destroying.
**Source:** Reverse brainstorming across all challenges
**Impact:** High
**Effort:** Medium (requires thoughtful error messages)
**Why it matters:** A framework's reputation is set by its worst failure mode, not its best demo. Every subsystem should fail loudly and clearly, with Autumn-branded error messages that tell the developer exactly what to do.

### Insight 3: The Layered Architecture Enables Progressive Magic
**Description:** Design each system with a "boring" default and a "magical" opt-in. This lets v0.1 ship safely while v0.2+ adds convenience without breaking existing users.
**Source:** SCAMPER alternatives across all challenges
**Impact:** High
**Effort:** Medium (requires trait-based abstraction at each layer)
**Why it matters:** This turns the single-developer constraint into an advantage. You don't have to solve everything at once. Each layer of magic is an independent addition, not a rewrite.

### Insight 4: The CLI Is the Magic Budget
**Description:** Move setup-time magic (downloads, scaffolding, tool management) to `autumn-cli`, keeping `cargo build` deterministic. The CLI is where you spend your "magic budget."
**Source:** Challenge 3 analysis, applicable to all
**Impact:** Medium
**Effort:** Low
**Why it matters:** `cargo build` is sacred in the Rust ecosystem. It must be fast, offline-capable, and deterministic. `autumn new` and `autumn setup` are Autumn's domain — they can do whatever they want. This separation respects Cargo's contract while still delivering the zero-config DX promise.

### Insight 5: Study Rocket, Not Just Spring Boot
**Description:** Rocket has solved many of Autumn's exact problems (route macros, registration, error handling) in Rust specifically. Its solutions are battle-tested across the Rust ecosystem's unique constraints.
**Source:** Six Thinking Hats (White Hat) across challenges
**Impact:** Medium
**Effort:** Low (just research)
**Why it matters:** Spring Boot is the *inspiration* but Rocket is the *prior art*. The `routes![]` macro, the `#[get]` attribute syntax, the managed state system — Rocket figured these out years ago. Study what worked, what didn't, and why. Don't reinvent unnecessarily.

---

## Impact on PRD Requirements

These insights should reshape several requirements:

1. **FR: Route registration** — should specify `routes![]` macro, not auto-discovery (auto-discovery becomes a future feature flag)
2. **FR: Proc macros** — should specify thin wrapper approach with `compile_error!` diagnostics, not deep function rewriting
3. **FR: Tailwind integration** — should specify CLI-managed download (`autumn new`/`autumn setup`), not `build.rs` download
4. **NFR: Build determinism** — add requirement that `cargo build` never accesses the network
5. **NFR: Error quality** — add requirement that all framework-generated errors include actionable messages pointing at user code
6. **FR: Feature flags** — add requirement for opt-in/opt-out features (Tailwind, auto-discovery)

---

## Statistics
- Total ideas: 42
- Categories: 3 (challenges) × 3 (techniques) = 9 analysis sections
- Key insights: 5
- Techniques applied: 3
- Direct PRD impacts identified: 6

## Recommended Next Steps

1. Proceed with `/bmad:research` to study Rocket, Loco, and Actix-web's approaches to these exact problems
2. Then return to `/bmad:prd` with refined requirements informed by both brainstorming and research

---

*Generated by BMAD Method v6 - Creative Intelligence*
